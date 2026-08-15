//! Request data passed from `Backend` to document-scoped method handlers.
//!
//! Every document handler starts with editor state captured by ingress before the async handler
//! runs. Ordinary document reads send the target text to the engine. Cross-file operations also
//! send the other open documents needed to check saved source ranges.
//!
//! Completion has a second context because it may move its cursor through a later edit and submit
//! another engine attempt. Keeping that state out of `DocumentMethodContext` makes the one-shot
//! flow easier to see.

use tower_lsp_server::{jsonrpc::Error, ls_types::*};

use rg_lsp_proto::{
    AnalysisOutcome, CompletionClientCapabilities, DocumentPositionSnapshot, DocumentQueryResult,
    DocumentRangeSnapshot, EditorDocumentSnapshot, GlobalOperationResult, GlobalPositionSnapshot,
};

use crate::completion_scheduler::CompletionRequest;
use crate::engine_client::EngineClient;
use crate::ingress::CapturedDocument;

use super::analysis_result::{self, DocumentQueryStatus, save_required, temporarily_unavailable};

/// Engine and captured editor state used by one document method attempt.
///
/// The capture was selected before this handler started. Input builders below build every engine
/// request from it, and the finish methods check the engine result against the same capture before
/// returning it to the editor.
#[derive(Clone, Debug)]
pub(crate) struct DocumentMethodContext {
    pub(crate) engine_client: EngineClient,
    captured: CapturedDocument,
}

impl DocumentMethodContext {
    pub(crate) fn new(engine_client: EngineClient, captured: CapturedDocument) -> Self {
        Self {
            engine_client,
            captured,
        }
    }

    pub(crate) fn captured_document(&self) -> &CapturedDocument {
        &self.captured
    }

    /// Build engine input containing only the captured target document.
    pub(crate) fn target_document(&self) -> Result<EditorDocumentSnapshot, Error> {
        self.captured
            .target_document(&self.engine_client)
            .map_err(|unavailable| {
                tracing::debug!(
                    path = ?unavailable.path().map(std::path::Path::display),
                    reason = unavailable.reason(),
                    "exact target document input is temporarily unavailable"
                );
                temporarily_unavailable(unavailable.reason())
            })
    }

    pub(crate) fn target_position(
        &self,
        position: Position,
    ) -> Result<DocumentPositionSnapshot, Error> {
        Ok(self.target_document()?.with_position(position))
    }

    pub(crate) fn target_range(&self, range: Range) -> Result<DocumentRangeSnapshot, Error> {
        Ok(self.target_document()?.with_range(range))
    }

    /// Build cross-file input with the target position and all relevant open documents.
    pub(crate) fn global_position(
        &self,
        position: Position,
    ) -> Result<GlobalPositionSnapshot, Error> {
        self.captured
            .global_position(&self.engine_client, position)
            .map_err(|unavailable| {
                tracing::debug!(
                    path = ?unavailable.path().map(std::path::Path::display),
                    reason = unavailable.reason(),
                    "global operation input is temporarily unavailable"
                );
                temporarily_unavailable(unavailable.reason())
            })
    }

    /// Return a global result only if all documents used by it are still unchanged.
    pub(crate) fn finish_global_operation<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<GlobalOperationResult<T>>>,
    ) -> Result<T, Error> {
        let result =
            analysis_result::global_operation_query(result, &self.captured)?.into_lsp_result()?;
        match result {
            GlobalOperationResult::Ready(value) => Ok(value),
            GlobalOperationResult::SaveRequired { path } => {
                tracing::debug!(
                    path = %path.display(),
                    "global operation requires saved source"
                );
                Err(save_required(&path))
            }
        }
    }

    /// Return a cross-file document result only while the captured open-document set is current.
    ///
    /// Definition queries may omit an unsafe dirty destination and report partial coverage. They
    /// do not require saving unrelated documents, but their locations still depend on the open
    /// documents captured by the request.
    pub(crate) fn finish_global_document_read<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<DocumentQueryResult<T>>>,
    ) -> Result<T, Error> {
        let result =
            analysis_result::global_operation_query(result, &self.captured)?.into_lsp_result()?;
        if result.coverage().is_partial() {
            tracing::debug!(
                path = %self.captured.document().path().display(),
                coverage = ?result.coverage(),
                "cross-file document read omitted source ranges that could not be mapped"
            );
        }
        Ok(result.into_value())
    }

    /// Return a document result after checking that its target is still unchanged.
    ///
    /// Partial coverage is still a valid best-effort result. Log it here so we can see when a query
    /// had to use saved project information instead of rebuilding every current body it needed.
    pub(crate) fn finish_document_read<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<DocumentQueryResult<T>>>,
    ) -> Result<T, Error> {
        let result =
            analysis_result::target_document_query(result, &self.captured)?.into_lsp_result()?;
        if result.coverage().is_partial() {
            tracing::debug!(
                path = %self.captured.document().path().display(),
                coverage = ?result.coverage(),
                "document read used current syntax and saved global semantics"
            );
        }
        Ok(result.into_value())
    }
}

/// Document context plus the state used only by the completion retry loop.
///
/// Unlike an ordinary document method, completion may replace its captured snapshot after an edit
/// and submit another engine query through the same logical request. This type makes that extra
/// state mandatory for completion without exposing it to every other handler.
#[derive(Clone, Debug)]
pub(crate) struct CompletionMethodContext {
    pub(crate) document: DocumentMethodContext,
    pub(crate) request: CompletionRequest,
    pub(crate) client_capabilities: CompletionClientCapabilities,
}

impl CompletionMethodContext {
    pub(crate) fn new(
        document: DocumentMethodContext,
        request: CompletionRequest,
        client_capabilities: CompletionClientCapabilities,
    ) -> Self {
        Self {
            document,
            request,
            client_capabilities,
        }
    }

    /// Replace the old snapshot after moving the completion cursor through accepted edits.
    pub(crate) fn replace_document(&mut self, captured: CapturedDocument) {
        self.document.captured = captured;
    }

    /// Build completion input from the target document only.
    pub(crate) fn input(&self, position: Position) -> Result<DocumentPositionSnapshot, Error> {
        self.document.target_position(position)
    }

    /// Check one completed engine attempt without deciding whether the LSP request should end.
    ///
    /// `EditorChanged` goes back to the completion loop, which can take a newer snapshot and try
    /// again. Engine aborts, transport failures, and mismatched response tags are still errors.
    pub(crate) fn finish_attempt<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<T>>,
    ) -> Result<DocumentQueryStatus<T>, Error> {
        analysis_result::target_document_query(result, self.document.captured_document())
    }
}
