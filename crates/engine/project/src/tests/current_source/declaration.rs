use rg_analysis::{CodeActionQuery, CompletionQuery, CompletionSource};
use rg_body_ir::{CurrentSourceBuildCheckpoint, CurrentSourceSelection};
use rg_parse::{Span, TextSpan};
use rg_std::CancellationToken;
use test_fixture::testonly::MarkedText;

use crate::{PackageResidencyPolicy, testonly::ProjectFixture};

const SAVED: &str = r#"
//- /Cargo.toml
[package]
name = "current_declarations"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["protocol"]

[dependencies]
protocol = { path = "protocol" }

[lib]
path = "src/shared.rs"

[[test]]
name = "shared"
path = "src/shared.rs"

//- /protocol/Cargo.toml
[package]
name = "protocol"
version = "0.1.0"
edition = "2024"

//- /protocol/src/lib.rs
pub trait Service<T> {
    fn implemented(&self, value: T);
    fn missing(&self, value: T);
    fn test_mode(&self) {}
}

//- /src/shared.rs
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
fn current_impl_queries_share_context_across_targets_and_residency() {
    for policy in [
        PackageResidencyPolicy::AllResident,
        PackageResidencyPolicy::AllOffloadable,
    ] {
        let fixture = ProjectFixture::build_with_package_residency_policy(SAVED, policy);
        let snapshot = fixture.project().snapshot();
        let context = snapshot
            .file_contexts_for_path(fixture.path("src/shared.rs"))
            .expect("shared file contexts should resolve")
            .pop()
            .expect("shared file has a context");
        let targets = context
            .crates
            .iter()
            .map(|crate_ref| (*crate_ref, context.file))
            .collect::<Vec<_>>();
        assert_eq!(targets.len(), 2);
        let before = fixture.project().stats();
        let current = current();

        for marker in ["header", "signature", "receiver", "members"] {
            let offset = u32::try_from(current.offset(marker)).expect("fixture offset fits u32");
            let source = snapshot
                .prepare_current_source(&targets, current.text())
                .expect("capture source");
            let (analysis, summary) = snapshot
                .analysis_for_current_source(
                    &targets,
                    source,
                    CurrentSourceSelection::AtOffset(offset),
                    CancellationToken::new(),
                    |_| Ok(()),
                )
                .expect("prepare declarations and bodies");
            assert!(summary.is_complete(), "{marker}: {summary:?}");
            let completion = CompletionSource::new(current.text(), offset)
                .expect("completion source should parse");

            for &(crate_ref, file) in &targets {
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
                        TextSpan {
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
        .file_contexts_for_path(fixture.path("src/shared.rs"))
        .expect("shared file contexts should resolve")
        .pop()
        .expect("shared file has a context");
    let targets = context
        .crates
        .iter()
        .map(|crate_ref| (*crate_ref, context.file))
        .collect::<Vec<_>>();
    let current = current();
    let source = snapshot
        .prepare_current_source(&targets, current.text())
        .expect("capture source");
    let txn = fixture
        .project()
        .state
        .read_txn()
        .expect("saved transaction should open");
    let db = txn.view_db();
    let impl_span = Span {
        text: TextSpan {
            start: current
                .offset("impl_start")
                .try_into()
                .expect("fixture offset fits u32"),
            end: current
                .offset("impl_end")
                .try_into()
                .expect("fixture offset fits u32"),
        },
    };

    // A cancelled build never installs partial declarations. The next preparation starts from
    // the same frozen readers and can still answer the complete member query.
    for (marker, cancel) in [("header", true), ("header", false), ("receiver", false)] {
        let offset = u32::try_from(current.offset(marker)).expect("fixture offset fits u32");
        let mut builder = db.current_source_builder(source.source());
        let mut enabled_targets = Vec::new();
        for &(crate_ref, file) in &targets {
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
            enabled_targets.push(
                package
                    .target(cargo_target)
                    .expect("target exists")
                    .enables_test_cfg(),
            );
            let result = builder.prepare_target(
                package,
                crate_ref,
                file,
                source
                    .declaration_associations(crate_ref.package, file)
                    .expect("associations exist"),
                true,
                CurrentSourceSelection::AtOffset(offset),
                db.trait_selection(crate_ref),
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
                break;
            }
            result.expect("current target should prepare");
        }
        if cancel {
            continue;
        }
        let (store, summary) = builder.finish().expect("freeze current context");
        assert!(summary.is_complete());
        let prepared = db.clone().with_current_source(store);
        assert!(enabled_targets.contains(&true) && enabled_targets.contains(&false));
        for (&(crate_ref, file), test_cfg) in targets.iter().zip(enabled_targets) {
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
}
