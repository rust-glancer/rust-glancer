use rg_analysis::{CodeActionQuery, CompletionQuery, CompletionSource};
use rg_ir_model::Span;
use rg_source::CapturedSource;
use rg_std::CancellationToken;
use rg_workspace::TargetKind;
use test_fixture::testonly::MarkedText;

use crate::testonly::ProjectFixture;

const SAVED: &str = r#"
//- /protocol/src/lib.rs crate:protocol root:protocol
pub trait Service<T> {
    fn implemented(&self, value: T);
    fn missing(&self, value: T);
    fn test_mode(&self) {}
}

//- /src/shared.rs crate:editor_declarations deps:protocol targets:lib=editor_declarations,test=shared
use protocol::Service;
pub struct Worker<T> { pub value: T }
"#;

#[test]
fn edited_impl_queries_use_each_selected_target() {
    let fixture = ProjectFixture::build(SAVED);
    let edited = MarkedText::parse(
        r#"
use protocol::Service;
pub struct Worker<T> { pub value: T }

impl<Local> Service<Local> for Wor$header$ker<Local> {
    fn implemented(&self, value: Lo$signature$cal) {
        self.val$receiver$ue;
    }

    #[cfg(test)]
    fn test_mode(&self) {}

    $members$
}
"#,
    );
    let source =
        CapturedSource::new(fixture.path("src/shared.rs"), edited.text()).expect("editor source");
    fixture
        .project()
        .with_editor_snapshot(&[source], CancellationToken::new(), |snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/shared.rs"))
                .expect("shared file contexts should resolve")
                .expect("shared file has a context");
            let owners = snapshot
                .crates_for_file(context.crate_ref.package, context.file)
                .expect("shared file owners should resolve");
            assert_eq!(owners.len(), 2);
            let before = fixture.project().generation_id();

            // One physical file has separate library/test interpretations of the same edit.
            for crate_ref in owners {
                let file = context.file;
                let is_test = fixture
                    .project()
                    .workspace()
                    .target(crate_ref)
                    .expect("fixture target")
                    .kind
                    == TargetKind::Test;
                for marker in ["header", "signature", "receiver", "members"] {
                    let offset =
                        u32::try_from(edited.offset(marker)).expect("fixture offset fits u32");
                    let analysis = snapshot.analysis();
                    let completion = CompletionSource::new(edited.text(), offset)
                        .expect("completion source should parse");

                    let labels = analysis
                        .completions_at(
                            CompletionQuery::new(crate_ref, file, offset)
                                .with_completion_source(&completion),
                        )
                        .expect("edited completion should resolve");
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
                        assert_eq!(
                            labels.iter().any(|item| item.label == "test_mode"),
                            !is_test,
                            "the test-only implementation changes missing-member completion: is_test={is_test}",
                        );
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

                    // The action must find the same impl from its header and its method body,
                    // and account for the members already written in this edit.
                    let actions = analysis
                        .code_actions(CodeActionQuery::new(
                            crate_ref,
                            file,
                            Span {
                                start: offset,
                                end: offset,
                            },
                            edited.text(),
                        ))
                        .expect("edited actions should resolve");
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
                fixture.project().generation_id(),
                before,
                "request declarations preserve the saved generation"
            );
            Ok(())
        })
        .expect("edited impl queries");
}
