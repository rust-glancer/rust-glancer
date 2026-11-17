//! Checks engine response scope before turning a query response into an LSP value.
//!
//! A successful document query carries the editor state used by the engine. This module compares
//! that state with the request capture and with live ingress state. If a later edit overtook the
//! query, it adds `QueryError::EditorChanged`; the engine cannot make that decision because it does
//! not own live editor state.

use std::borrow::Cow;

use rg_lsp_proto::{EngineError, QueryError, QueryScope, QueryValue, TargetDocumentRevision};
use tower_lsp_server::jsonrpc::{Error, ErrorCode};

use crate::ingress::{CapturedDocument, CapturedEditor};

/// A workspace request has the same editor dependency as a document request, but no cursor.
pub(crate) fn validate_workspace<T>(
    result: Result<QueryValue<T>, QueryError>,
    captured: &CapturedEditor,
) -> Result<T, QueryError> {
    validate_editor(result, captured, None)
}

pub(super) fn validate_document<T>(
    result: Result<QueryValue<T>, QueryError>,
    captured: &CapturedDocument,
) -> Result<T, QueryError> {
    validate_editor(
        result,
        captured.editor(),
        Some(captured.document().target()),
    )
}

fn validate_editor<T>(
    result: Result<QueryValue<T>, QueryError>,
    captured: &CapturedEditor,
    expected_target: Option<&TargetDocumentRevision>,
) -> Result<T, QueryError> {
    let (value, scope) = result?.into_parts();
    let QueryScope::Editor {
        target,
        open_documents_revision,
    } = scope
    else {
        return Err(internal_query_error(
            "semantic query response has the wrong publication scope",
        ));
    };
    if target.as_ref() != expected_target || open_documents_revision != captured.revision() {
        return Err(internal_query_error(
            "semantic query response does not match its captured editor state",
        ));
    }
    if !captured.is_current() {
        return Err(QueryError::EditorChanged);
    }
    Ok(value)
}

/// Check a result that depends on its target document but not on other open documents.
pub(super) fn validate_target_document<T>(
    result: Result<QueryValue<T>, QueryError>,
    captured: &CapturedDocument,
) -> Result<T, QueryError> {
    let response = result?;
    let (value, scope) = response.into_parts();
    let QueryScope::TargetDocument(target) = scope else {
        return Err(internal_query_error(
            "target-document query response has the wrong publication scope",
        ));
    };
    validate_target(&target, captured)?;
    if !captured.is_target_current(&target) {
        tracing::debug!(
            path = %target.path().display(),
            session = target.session().get(),
            revision = target.revision().get(),
            "target-only analysis result was overtaken before LSP publication"
        );
        return Err(QueryError::EditorChanged);
    }
    Ok(value)
}

fn validate_target(
    target: &TargetDocumentRevision,
    captured: &CapturedDocument,
) -> Result<(), QueryError> {
    if captured.document().target() != target {
        return Err(internal_query_error(
            "query response target does not match its captured request",
        ));
    }
    Ok(())
}

fn internal_query_error(message: impl Into<String>) -> QueryError {
    QueryError::Internal(EngineError::new(message))
}

/// Convert the shared query error model into the JSON-RPC response expected by an editor.
pub(crate) fn into_lsp_error(error: QueryError) -> Error {
    let message = error.to_string();
    match error {
        QueryError::SavedSourceChanged | QueryError::TemporarilyUnavailable => {
            tracing::debug!(%message, "analysis request stopped before semantic publication");
            temporarily_unavailable(&message)
        }
        QueryError::EditorChanged => temporarily_unavailable(&message),
        QueryError::SaveRequired { path } => {
            tracing::debug!(
                path = %path.display(),
                "workspace edit requires saved text for this client"
            );
            temporarily_unavailable(&message)
        }
        QueryError::Internal(_) => Error {
            code: ErrorCode::InternalError,
            message: Cow::Owned(message),
            data: None,
        },
    }
}

/// Preserve an internal error's context chain in one JSON-RPC error message.
pub(crate) fn internal_error(error: anyhow::Error) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: Cow::Owned(format!("{error:#}")),
        data: None,
    }
}

/// Report that a request's captured input can no longer be used.
///
/// ContentModified does not guarantee a client retry. A handler that follows edits must handle
/// EditorChanged before this conversion, keeping its logical request pending while it tries again.
pub(crate) fn temporarily_unavailable(reason: &str) -> Error {
    let mut error = Error::content_modified();
    error.message = Cow::Owned(reason.to_string());
    error
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rg_lsp_proto::{QueryError, QueryScope, QueryValue};
    use tower_lsp_server::{
        gen_lsp_types::{TextDocumentContentChangeEvent, TextDocumentContentChangeWholeDocument},
        jsonrpc::ErrorCode,
    };

    use super::{
        internal_error, into_lsp_error, validate_document, validate_target_document,
        validate_workspace,
    };
    use crate::{ingress::EditorStateHandle, tests::synthetic_test_path};

    #[test]
    fn internal_error_preserves_context_chain() {
        let error = anyhow::anyhow!("engine response channel closed")
            .context("while receiving engine response")
            .context("while handling hover");

        let message = internal_error(error).message;

        assert_eq!(
            message.as_ref(),
            "while handling hover: while receiving engine response: engine response channel closed",
        );
    }

    #[test]
    fn workspace_query_checks_the_complete_editor_revision() {
        let editor = EditorStateHandle::default();
        let path = synthetic_test_path("workspace/src/lib.rs");
        editor.open(path.clone(), Some(1), "fn first() {}".to_string());
        let captured = editor.capture();
        let scope = QueryScope::Editor {
            target: None,
            open_documents_revision: captured.revision(),
        };
        assert!(
            validate_workspace(
                Ok(QueryValue::new(Vec::<usize>::new(), scope.clone())),
                &captured
            )
            .expect("empty result")
            .is_empty()
        );
        editor
            .change(&path, Some(2), &[full_change("fn second() {}")])
            .expect("edit");
        assert_eq!(
            validate_workspace(Ok(QueryValue::new(1_u8, scope)), &captured),
            Err(QueryError::EditorChanged)
        );
    }

    #[test]
    fn document_query_reports_an_edit_that_arrived_after_analysis() {
        let path = synthetic_test_path("workspace/src/lib.rs");
        let editor = EditorStateHandle::default();
        editor.open(path.clone(), Some(1), "fn first() {}".to_string());
        let captured = editor
            .document(Some(path.clone()))
            .expect("opened document should be captured");
        let scope = QueryScope::Editor {
            target: Some(captured.document().target().clone()),
            open_documents_revision: captured.open_documents_revision(),
        };

        assert!(
            editor
                .change(&path, Some(2), &[full_change("fn second() {}")])
                .expect("full change should apply")
        );
        let error = validate_document(Ok(QueryValue::new(17_u8, scope)), &captured)
            .expect_err("a later editor change should invalidate the value");

        assert_eq!(error, QueryError::EditorChanged);
    }

    #[test]
    fn target_document_query_ignores_a_later_sibling_edit() {
        let target = synthetic_test_path("workspace/src/lib.rs");
        let sibling = synthetic_test_path("workspace/src/sibling.rs");
        let editor = EditorStateHandle::default();
        editor.open(target.clone(), Some(1), "fn target() {}".to_string());
        editor.open(sibling.clone(), Some(1), "fn sibling() {}".to_string());
        let captured = editor
            .document(Some(target))
            .expect("target document should be captured");
        let scope = QueryScope::TargetDocument(captured.document().target().clone());

        assert!(
            editor
                .change(&sibling, Some(2), &[full_change("fn changed() {}")])
                .expect("full sibling change should apply")
        );
        let result = validate_target_document(Ok(QueryValue::new(17_u8, scope)), &captured)
            .expect("a sibling edit must not invalidate target-only analysis");

        assert_eq!(result, 17);
    }

    #[test]
    fn target_document_query_rejects_an_action_overtaken_by_a_new_version() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let editor = EditorStateHandle::default();
        editor.open(path.clone(), Some(7), "fn before() {}".to_string());
        let captured = editor
            .document(Some(path.clone()))
            .expect("action document should be captured");
        let scope = QueryScope::TargetDocument(captured.document().target().clone());

        assert!(
            editor
                .change(&path, Some(8), &[full_change("fn after() {}")])
                .expect("later action document change should apply")
        );
        let error = validate_target_document(Ok(QueryValue::new(vec!["action"], scope)), &captured)
            .expect_err("an action for an older document version must not be published");

        assert_eq!(error, QueryError::EditorChanged);
    }

    #[test]
    fn maps_typed_retryable_query_errors_to_content_modified() {
        let fixtures = [
            (
                QueryError::EditorChanged,
                "analysis request was superseded by newer editor state",
            ),
            (
                QueryError::SavedSourceChanged,
                "saved source changed while analysis was running",
            ),
            (
                QueryError::TemporarilyUnavailable,
                "analysis input is temporarily unavailable",
            ),
            (
                QueryError::SaveRequired {
                    path: PathBuf::from("/workspace/src/lib.rs"),
                },
                "save `/workspace/src/lib.rs` before running this operation",
            ),
        ];

        for (query_error, expected_message) in fixtures {
            let error = into_lsp_error(query_error);

            assert_eq!(error.code, ErrorCode::ContentModified);
            assert_eq!(error.message.as_ref(), expected_message);
        }
    }

    fn full_change(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeWholeDocument {
            text: text.to_string(),
        }
        .into()
    }
}
