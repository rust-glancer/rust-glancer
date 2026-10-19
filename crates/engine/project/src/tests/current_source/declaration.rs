use rg_analysis::{CodeActionQuery, CompletionQuery, CompletionSource};
use rg_body_ir::{CurrentSourceBuildCheckpoint, CurrentSourceSelection};
use rg_ir_model::Span;
use rg_std::CancellationToken;
use test_fixture::testonly::MarkedText;

use crate::{PackageResidencyPolicy, testonly::ProjectFixture};

const SAVED: &str = r#"
//- /protocol/src/lib.rs crate:protocol root:protocol
pub trait Service<T> {
    fn implemented(&self, value: T);
    fn missing(&self, value: T);
    fn test_mode(&self) {}
}

//- /src/shared.rs crate:current_declarations deps:protocol targets:lib=current_declarations,test=shared
use protocol::Service;
pub struct Worker<T> { pub value: T }
"#;

fn current() -> MarkedText {
    MarkedText::parse(
        r#"
use protocol::Service;
pub struct Worker<T> { pub value: T }

$impl_start$impl<Local> Service<Local> for Wor$header$ker<Local> {
    fn implemented(&self, value: Lo$signature$cal) {
        self.val$receiver$ue;
    }

    #[cfg(test)]
    fn test_mode(&self) {}

    $members$
}$impl_end$
"#,
    )
}

#[test]
fn current_impl_queries_use_each_selected_target_and_residency() {
    for policy in [
        PackageResidencyPolicy::AllResident,
        PackageResidencyPolicy::AllOffloadable,
    ] {
        let fixture = ProjectFixture::build_with_package_residency_policy(SAVED, policy);
        let snapshot = fixture.project().snapshot();
        let context = snapshot
            .file_context_for_path(fixture.path("src/shared.rs"))
            .expect("shared file contexts should resolve")
            .expect("shared file has a context");
        let owners = snapshot
            .crates_for_file(context.crate_ref.package, context.file)
            .expect("shared file owners should resolve");
        assert_eq!(owners.len(), 2);
        let before = fixture.project().stats();
        let current = current();

        // Each request prepares one owner; cfg and source associations never merge.
        for crate_ref in owners {
            let target = (crate_ref, context.file);
            for marker in ["header", "signature", "receiver", "members"] {
                let offset =
                    u32::try_from(current.offset(marker)).expect("fixture offset fits u32");
                let source = snapshot
                    .prepare_current_source(
                        target,
                        current.text(),
                        &rg_std::CancellationToken::new(),
                    )
                    .expect("capture source");
                let (analysis, summary) = snapshot
                    .analysis_for_current_source(
                        target,
                        source,
                        CurrentSourceSelection::AtOffset(offset),
                        CancellationToken::new(),
                        |_| Ok(()),
                    )
                    .expect("prepare declarations and bodies");
                assert!(summary.is_complete(), "{marker}: {summary:?}");
                let completion = CompletionSource::new(current.text(), offset)
                    .expect("completion source should parse");

                let (crate_ref, file) = target;
                let labels = analysis
                    .completions_at(
                        CompletionQuery::new(crate_ref, file, offset)
                            .with_completion_source(&completion),
                    )
                    .expect("current completion should resolve");
                let expected = match marker {
                    "header" => "Worker",
                    "signature" => "Local",
                    "receiver" => "value",
                    "members" => "missing",
                    _ => unreachable!("markers are enumerated above"),
                };
                let rows = labels
                    .iter()
                    .filter(|item| item.label == expected)
                    .collect::<Vec<_>>();
                assert_eq!(
                    rows.len(),
                    1,
                    "{marker} should have one {expected} candidate: {labels:?}"
                );
                if marker == "members" {
                    assert!(
                        rows[0]
                            .detail
                            .as_deref()
                            .is_some_and(|detail| detail.contains("value: Local"))
                    );
                }

                if matches!(marker, "header" | "signature" | "receiver") {
                    assert!(
                        analysis
                            .hover(crate_ref, file, offset)
                            .expect("hover should resolve")
                            .is_some()
                    );
                    if marker == "signature" {
                        assert!(
                            analysis
                                .type_at(crate_ref, file, offset)
                                .expect("generic type should resolve")
                                .is_some()
                        );
                    } else {
                        assert_eq!(
                            analysis
                                .goto_definition(crate_ref, file, offset)
                                .expect("navigation should resolve")
                                .len(),
                            1,
                            "{marker} has one declaration"
                        );
                    }
                }

                // Ask for missing members from both a declaration-only header and a prepared
                // method body. The method itself must remain accounted for in either context.
                let actions = analysis
                    .code_actions(CodeActionQuery::new(
                        crate_ref,
                        file,
                        Span {
                            start: offset,
                            end: offset,
                        },
                        current.text(),
                    ))
                    .expect("current actions should resolve");
                let action = actions
                    .iter()
                    .find(|action| action.title == "Implement missing trait members")
                    .expect("the edited impl has a missing member");
                let inserted = action
                    .edits
                    .iter()
                    .map(|edit| edit.new_text.as_str())
                    .collect::<String>();
                assert!(
                    inserted.contains("fn missing(&self, value: Local)"),
                    "{inserted}"
                );
                assert!(
                    !inserted.contains("fn implemented"),
                    "the prepared method is already implemented: {inserted}"
                );
            }
        }
        assert_eq!(
            fixture.project().stats(),
            before,
            "request declarations do not change retained stores"
        );
    }
}

#[test]
fn complete_impl_projection_applies_each_targets_cfg_and_survives_cancelled_preparation() {
    let fixture = ProjectFixture::build(SAVED);
    let snapshot = fixture.project().snapshot();
    let context = snapshot
        .file_context_for_path(fixture.path("src/shared.rs"))
        .expect("shared file contexts should resolve")
        .expect("shared file has a context");
    let owners = snapshot
        .crates_for_file(context.crate_ref.package, context.file)
        .expect("shared file owners should resolve");
    let current = current();
    let txn = fixture
        .project()
        .state
        .read_txn(rg_std::CancellationToken::new())
        .expect("saved transaction should open");
    let db = txn.view_db();
    let impl_span = Span {
        start: current
            .offset("impl_start")
            .try_into()
            .expect("fixture offset fits u32"),
        end: current
            .offset("impl_end")
            .try_into()
            .expect("fixture offset fits u32"),
    };

    // A cancelled build never installs partial declarations. Every retry starts a fresh target build.
    let mut enabled_targets = Vec::new();
    for crate_ref in owners {
        let file = context.file;
        let source = snapshot
            .prepare_current_source((crate_ref, file), current.text(), &CancellationToken::new())
            .expect("capture source");
        let package = fixture
            .project()
            .state
            .parse_db()
            .package(crate_ref.package.0)
            .expect("parse package exists");
        let defs = fixture
            .project()
            .state
            .def_map
            .resident_package(crate_ref.package)
            .expect("definitions exist");
        let cargo_target = defs
            .crate_data(crate_ref.crate_id)
            .expect("crate exists")
            .cargo_target();
        let test_cfg = package
            .target(cargo_target)
            .expect("target exists")
            .enables_test_cfg();
        enabled_targets.push(test_cfg);
        for (marker, cancel) in [("header", true), ("header", false), ("receiver", false)] {
            let offset = u32::try_from(current.offset(marker)).expect("fixture offset fits u32");
            let result = db.current_source_builder(source.source()).build_target(
                package,
                crate_ref,
                file,
                source
                    .declaration_associations(crate_ref.package, file)
                    .expect("associations exist"),
                true,
                CurrentSourceSelection::AtOffset(offset),
                |checkpoint| {
                    anyhow::ensure!(
                        !cancel || checkpoint != CurrentSourceBuildCheckpoint::DeclarationsPrepared,
                        "cancel declaration preparation"
                    );
                    Ok(())
                },
            );
            if cancel {
                assert!(
                    format!("{:#}", result.expect_err("preparation should cancel"))
                        .contains("cancel declaration preparation")
                );
                continue;
            }
            let (store, summary) = result.expect("current target should prepare");
            assert!(summary.is_complete());
            let prepared = db.clone().with_current_source(store);
            let impl_ref = prepared
                .selected_current_impl(crate_ref, file, impl_span)
                .expect("complete impl was selected");
            let members = rg_ir_view::trait_impl::TraitImplView::new(&prepared)
                .missing_members_for_prepared_impl(impl_ref)
                .expect("member projection should resolve");
            let labels = members
                .iter()
                .map(|member| member.label())
                .collect::<Vec<_>>();
            let expected = if test_cfg {
                vec!["missing"]
            } else {
                vec!["missing", "test_mode"]
            };
            assert_eq!(labels, expected, "{marker}, cfg(test)={test_cfg}");
        }
    }
    assert!(enabled_targets.contains(&true) && enabled_targets.contains(&false));
}
