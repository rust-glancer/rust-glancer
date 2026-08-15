//! Checks engine response ids before turning a response into an LSP value.
//!
//! Workspace queries use only a saved project id. A document query also returns the target session
//! and text revision that it used. Cross-file operations return one more id for the full captured
//! set of open documents. This module checks those ids against both the original request and the
//! live editor state.
//!
//! Completion treats `EditorChanged` specially: it can capture the newer text and retry inside the
//! same client request. Other document handlers return `ContentModified` and let the editor decide
//! whether to send another request.

use std::{borrow::Cow, path::Path};

use tower_lsp_server::jsonrpc::{Error, ErrorCode};

use rg_lsp_proto::{AnalysisAbort, AnalysisOutcome, AnalysisReady};

use crate::ingress::CapturedDocument;

const OVERTAKEN_DOCUMENT_REASON: &str = "analysis request was superseded by newer editor state";

/// Whether a checked document result still describes the live editor state.
pub(crate) enum DocumentQueryStatus<T> {
    /// The response tags match the capture and the editor has not changed since.
    Current(T),
    /// The response tags are valid, but a later editor change made the value old.
    EditorChanged,
}

impl<T> DocumentQueryStatus<T> {
    /// Return a current value, or ask the LSP client to retry after an editor change.
    pub(super) fn into_lsp_result(self) -> Result<T, Error> {
        match self {
            Self::Current(value) => Ok(value),
            Self::EditorChanged => Err(temporarily_unavailable(OVERTAKEN_DOCUMENT_REASON)),
        }
    }
}

/// Return a workspace value after verifying that the engine used saved-project input.
///
/// A document-tagged response here is an internal routing error. Engine aborts and transport
/// failures become JSON-RPC errors rather than valid empty workspace results.
pub(crate) fn workspace_query<T>(result: anyhow::Result<AnalysisOutcome<T>>) -> Result<T, Error> {
    let ready = ready_result(result)?;
    if ready.input().target_document().is_some() {
        return Err(internal_error(anyhow::anyhow!(
            "workspace analysis response unexpectedly targets a document"
        )));
    }
    Ok(ready.into_value())
}

/// Check that a global result belongs to this request and that none of its documents changed.
///
/// Mismatched response tags mean the engine answered a different request and are internal errors.
/// A matching response may still be `EditorChanged` when another edit arrived while analysis was
/// running.
pub(super) fn global_operation_query<T>(
    result: anyhow::Result<AnalysisOutcome<T>>,
    captured: &CapturedDocument,
) -> Result<DocumentQueryStatus<T>, Error> {
    let ready = ready_result(result)?;
    let Some(target) = ready.input().target_document() else {
        return Err(internal_error(anyhow::anyhow!(
            "document analysis response has no target revision"
        )));
    };
    if captured.document().target() != target {
        return Err(internal_error(anyhow::anyhow!(
            "analysis response target does not match its captured request"
        )));
    }
    let Some(open_documents_revision) = ready.input().open_documents_revision() else {
        return Err(internal_error(anyhow::anyhow!(
            "global operation response has no ingress-state epoch"
        )));
    };
    if captured.open_documents_revision() != open_documents_revision {
        return Err(internal_error(anyhow::anyhow!(
            "global operation response does not match its captured ingress-state epoch"
        )));
    }
    if !captured.is_global_operation_current(target, open_documents_revision) {
        tracing::debug!(
            path = %target.path().display(),
            session = target.session().get(),
            revision = target.revision().get(),
            "analysis result was overtaken before LSP publication"
        );
        return Ok(DocumentQueryStatus::EditorChanged);
    }
    Ok(DocumentQueryStatus::Current(ready.into_value()))
}

/// Check a result that depends on its target document but not on other open documents.
///
/// Document-local reads use this path. A sibling may change while analysis is running without
/// making the result stale; replacing the target text, closing it, or reopening its path still
/// prevents publication.
pub(super) fn target_document_query<T>(
    result: anyhow::Result<AnalysisOutcome<T>>,
    captured: &CapturedDocument,
) -> Result<DocumentQueryStatus<T>, Error> {
    let ready = ready_result(result)?;
    let Some(target) = ready.input().target_document() else {
        return Err(internal_error(anyhow::anyhow!(
            "target-only analysis response has no target revision"
        )));
    };
    if captured.document().target() != target {
        return Err(internal_error(anyhow::anyhow!(
            "target-only analysis response does not match its captured request"
        )));
    }
    if ready.input().open_documents_revision().is_some() {
        return Err(internal_error(anyhow::anyhow!(
            "target-only analysis response unexpectedly depends on open sibling documents"
        )));
    }
    if !captured.is_target_current(target) {
        tracing::debug!(
            path = %target.path().display(),
            session = target.session().get(),
            revision = target.revision().get(),
            "target-only analysis result was overtaken before LSP publication"
        );
        return Ok(DocumentQueryStatus::EditorChanged);
    }
    Ok(DocumentQueryStatus::Current(ready.into_value()))
}

/// Extract the successful engine value while preserving aborts and failures as errors.
fn ready_result<T>(result: anyhow::Result<AnalysisOutcome<T>>) -> Result<AnalysisReady<T>, Error> {
    match result {
        Ok(AnalysisOutcome::Ready(ready)) => Ok(ready),
        Ok(AnalysisOutcome::Aborted(abort)) => Err(logged_abort_error(abort)),
        Err(error) => Err(internal_error(error)),
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

/// Tell the client which document must be saved before a cross-file operation can run.
pub(super) fn save_required(path: &Path) -> Error {
    let mut error = Error::content_modified();
    error.message = Cow::Owned(format!(
        "save `{}` before running this operation",
        path.display()
    ));
    error
}

fn logged_abort_error(abort: AnalysisAbort) -> Error {
    tracing::debug!(
        ?abort,
        "analysis request aborted before semantic publication"
    );
    let mut error = Error::content_modified();
    error.message = Cow::Borrowed(abort.description());
    error
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rg_lsp_proto::{AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady};
    use tower_lsp_server::{jsonrpc::ErrorCode, ls_types::TextDocumentContentChangeEvent};

    use super::{
        DocumentQueryStatus, OVERTAKEN_DOCUMENT_REASON, global_operation_query, internal_error,
        save_required, target_document_query, workspace_query,
    };
    use crate::ingress::EditorStateHandle;

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
        assert!(
            !message.contains('\n'),
            "alternate anyhow display should keep context chains on one line",
        );
    }

    #[test]
    fn workspace_query_preserves_valid_empty_values() {
        let result = workspace_query::<Vec<usize>>(Ok(AnalysisOutcome::Ready(AnalysisReady::new(
            Vec::new(),
            AnalysisInput::for_saved_project(5),
        ))))
        .expect("valid empty analysis should remain successful");

        assert!(result.is_empty());
    }

    #[test]
    fn save_required_is_a_client_neutral_content_modified_response() {
        let error = save_required(PathBuf::from("/workspace/src/lib.rs").as_path());

        assert_eq!(error.code, ErrorCode::ContentModified);
        assert_eq!(
            error.message.as_ref(),
            "save `/workspace/src/lib.rs` before running this operation",
        );
    }

    #[test]
    fn document_query_reports_an_edit_that_arrived_after_analysis() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let editor = EditorStateHandle::default();
        editor.open(path.clone(), Some(1), "fn first() {}".to_string());
        let captured = editor
            .document(Some(path.clone()))
            .expect("opened document should be captured");
        let input = AnalysisInput::for_global_operation(
            11,
            captured.open_documents_revision(),
            captured.document().target().clone(),
        );
        let ready = AnalysisOutcome::Ready(AnalysisReady::new(17_u8, input));

        assert!(
            editor
                .change(&path, Some(2), &[full_change("fn second() {}")])
                .expect("full change should apply")
        );
        let status = global_operation_query(Ok(ready), &captured)
            .expect("a later editor change is not an engine response error");

        assert!(matches!(&status, DocumentQueryStatus::EditorChanged));
        let error = status
            .into_lsp_result()
            .expect_err("ordinary document queries must ask the client to retry");
        assert_eq!(error.code, ErrorCode::ContentModified);
        assert_eq!(error.message.as_ref(), OVERTAKEN_DOCUMENT_REASON);
    }

    #[test]
    fn target_document_query_ignores_a_later_sibling_edit() {
        let target = PathBuf::from("/workspace/src/lib.rs");
        let sibling = PathBuf::from("/workspace/src/sibling.rs");
        let editor = EditorStateHandle::default();
        editor.open(target.clone(), Some(1), "fn target() {}".to_string());
        editor.open(sibling.clone(), Some(1), "fn sibling() {}".to_string());
        let captured = editor
            .document(Some(target))
            .expect("target document should be captured");
        let input = AnalysisInput::for_target_document(11, captured.document().target().clone());
        let ready = AnalysisOutcome::Ready(AnalysisReady::new(17_u8, input));

        assert!(
            editor
                .change(&sibling, Some(2), &[full_change("fn changed() {}")])
                .expect("full sibling change should apply")
        );
        let status = target_document_query(Ok(ready), &captured)
            .expect("a sibling edit must not invalidate target-only analysis");

        assert!(matches!(status, DocumentQueryStatus::Current(17)));
    }

    #[test]
    fn workspace_query_maps_operational_aborts_without_feature_defaults() {
        let cases = [
            (AnalysisAbort::SourceChanged, ErrorCode::ContentModified),
            (
                AnalysisAbort::TemporarilyUnavailable,
                ErrorCode::ContentModified,
            ),
        ];

        for (abort, expected_code) in cases {
            let error = workspace_query::<Vec<usize>>(Ok(AnalysisOutcome::Aborted(abort)))
                .expect_err("operational abort should remain distinguishable from valid empty");

            assert_eq!(error.code, expected_code);
            assert_eq!(error.message.as_ref(), abort.description());
        }
    }

    fn full_change(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }
    }
}
