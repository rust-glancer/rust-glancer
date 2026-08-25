//! Checks engine response scope before turning a query response into an LSP value.
//!
//! A successful document query carries the editor state used by the engine. This module compares
//! that state with the request capture and with live ingress state. If a later edit overtook the
//! query, it adds `QueryError::EditorChanged`; the engine cannot make that decision because it does
//! not own live editor state.

use std::borrow::Cow;

use rg_lsp_proto::{EngineError, QueryError, QueryScope, QueryValue, TargetDocumentRevision};
use tower_lsp_server::jsonrpc::{Error, ErrorCode};

use crate::ingress::CapturedDocument;

/// Return a workspace value after verifying that the engine used only saved-project state.
pub(crate) fn validate_workspace<T>(
    result: Result<QueryValue<T>, QueryError>,
) -> Result<T, QueryError> {
    let response = result?;
    let (value, scope) = response.into_parts();
    if !matches!(scope, QueryScope::SavedProject) {
        return Err(internal_query_error(
            "workspace query response unexpectedly depends on editor state",
        ));
    }
    Ok(value)
}

/// Check that a global operation belongs to this request and that none of its documents changed.
pub(super) fn validate_global_operation<T>(
    result: Result<QueryValue<T>, QueryError>,
    captured: &CapturedDocument,
) -> Result<T, QueryError> {
    let response = result?;
    let (value, scope) = response.into_parts();
    let QueryScope::GlobalOperation {
        target,
        open_documents_revision,
    } = scope
    else {
        return Err(internal_query_error(
            "global query response has the wrong publication scope",
        ));
    };
    validate_target(&target, captured)?;
    if captured.open_documents_revision() != open_documents_revision {
        return Err(internal_query_error(
            "global query response does not match its captured ingress-state epoch",
        ));
    }
    if !captured.is_global_operation_current(&target, open_documents_revision) {
        tracing::debug!(
            path = %target.path().display(),
            session = target.session().get(),
            revision = target.revision().get(),
            "analysis result was overtaken before LSP publication"
        );
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
                "global operation requires saved source"
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

/// Ask the editor to retry a request whose captured input can no longer be used.
pub(crate) fn temporarily_unavailable(reason: &str) -> Error {
    let mut error = Error::content_modified();
    error.message = Cow::Owned(reason.to_string());
    error
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rg_lsp_proto::{QueryError, QueryScope, QueryValue};
    use tower_lsp_server::{jsonrpc::ErrorCode, ls_types::TextDocumentContentChangeEvent};

    use super::{
        internal_error, into_lsp_error, validate_global_operation, validate_target_document,
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
    fn workspace_query_preserves_valid_empty_values() {
        let result: Result<QueryValue<Vec<usize>>, QueryError> =
            Ok(QueryValue::new(Vec::new(), QueryScope::SavedProject));

        assert!(
            validate_workspace(result)
                .expect("valid empty analysis should remain successful")
                .is_empty()
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
        let scope = QueryScope::GlobalOperation {
            target: captured.document().target().clone(),
            open_documents_revision: captured.open_documents_revision(),
        };

        assert!(
            editor
                .change(&path, Some(2), &[full_change("fn second() {}")])
                .expect("full change should apply")
        );
        let error = validate_global_operation(Ok(QueryValue::new(17_u8, scope)), &captured)
            .expect_err("a later editor change should invalidate the value");

        assert_eq!(error, QueryError::EditorChanged);
        let expected_message = error.to_string();
        let error = into_lsp_error(error);
        assert_eq!(error.code, ErrorCode::ContentModified);
        assert_eq!(error.message.as_ref(), expected_message);
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
    fn retryable_errors_remain_distinct_from_valid_empty_results() {
        for query_error in [
            QueryError::SavedSourceChanged,
            QueryError::TemporarilyUnavailable,
        ] {
            let expected_message = query_error.to_string();
            let error = into_lsp_error(query_error);
            assert_eq!(error.code, ErrorCode::ContentModified);
            assert_eq!(error.message.as_ref(), expected_message);
        }
    }

    #[test]
    fn save_required_names_the_document_to_publish() {
        let error = into_lsp_error(QueryError::SaveRequired {
            path: PathBuf::from("/workspace/src/lib.rs"),
        });

        assert_eq!(error.code, ErrorCode::ContentModified);
        assert_eq!(
            error.message.as_ref(),
            "save `/workspace/src/lib.rs` before running this operation",
        );
    }

    fn full_change(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }
    }
}
