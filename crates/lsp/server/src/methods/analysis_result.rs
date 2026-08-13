//! Converts engine analysis outcomes into values or JSON-RPC errors.
//!
//! Engine responses take one of two routes:
//!
//! - Workspace queries must carry saved-project input and do not depend on live editor text.
//! - Document queries must identify the exact captured document and global editor revision used by
//!   the engine. After checking those tags, the server also checks whether that editor state is
//!   still live.
//!
//! Completion needs to distinguish an ordinary failure from `EditorChanged`, because it can take
//! a newer snapshot and retry inside the same client request. Other document handlers turn that
//! status into `ContentModified` in `DocumentMethodContext::finish_query`.

use std::borrow::Cow;

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

/// Check a document value against both its captured input and the live editor state.
///
/// Mismatched response tags mean the engine answered a different request and are internal errors.
/// A matching response may still be `EditorChanged` when another edit arrived while analysis was
/// running.
pub(super) fn document_query<T>(
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
    let Some(editor_revision) = ready.input().editor_snapshot_revision() else {
        return Err(internal_error(anyhow::anyhow!(
            "document analysis response has no editor snapshot revision"
        )));
    };
    if captured.editor_revision() != editor_revision {
        return Err(internal_error(anyhow::anyhow!(
            "analysis response editor revision does not match its captured request"
        )));
    }
    if !captured.is_current(target, editor_revision) {
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

    use rg_lsp_proto::{
        AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady, AnalysisScope,
    };
    use tower_lsp_server::{jsonrpc::ErrorCode, ls_types::TextDocumentContentChangeEvent};

    use super::{
        DocumentQueryStatus, OVERTAKEN_DOCUMENT_REASON, document_query, internal_error,
        workspace_query,
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
    fn document_query_reports_an_edit_that_arrived_after_analysis() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let editor = EditorStateHandle::default();
        editor.open(path.clone(), Some(1), "fn first() {}".to_string());
        let captured = editor
            .document(Some(path.clone()))
            .expect("opened document should be captured");
        let input = AnalysisInput::for_target_revision(
            11,
            captured.editor_revision(),
            captured.document().target().clone(),
            AnalysisScope::ChangedPackages,
        );
        let ready = AnalysisOutcome::Ready(AnalysisReady::new(17_u8, input));

        assert!(
            editor
                .change(&path, Some(2), &[full_change("fn second() {}")])
                .expect("full change should apply")
        );
        let status = document_query(Ok(ready), &captured)
            .expect("a later editor change is not an engine response error");

        assert!(matches!(&status, DocumentQueryStatus::EditorChanged));
        let error = status
            .into_lsp_result()
            .expect_err("ordinary document queries must ask the client to retry");
        assert_eq!(error.code, ErrorCode::ContentModified);
        assert_eq!(error.message.as_ref(), OVERTAKEN_DOCUMENT_REASON);
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
