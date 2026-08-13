//! Request data passed from `Backend` to document-scoped method handlers.
//!
//! Ordinary document methods need an engine and one immutable editor snapshot. Completion needs
//! those same values plus request ownership, bounded scheduling, and the client features used to
//! render completion items. Keeping these as two concrete types prevents workspace methods and
//! ordinary document methods from carrying optional completion-only state.

use tower_lsp_server::{jsonrpc::Error, ls_types::*};

use rg_lsp_proto::{
    AnalysisOutcome, CompletionClientCapabilities, DocumentAnalysisSnapshot,
    DocumentPositionSnapshot, DocumentRangeSnapshot,
};

use crate::completion_scheduler::CompletionRequest;
use crate::engine_client::EngineClient;
use crate::ingress::CapturedDocument;

use super::analysis_result::{self, DocumentQueryStatus, temporarily_unavailable};

/// Engine and immutable editor snapshot used by one ordinary document method.
///
/// The snapshot was selected before this handler started. Input builders below derive every engine
/// request from that snapshot, and `finish_query` checks the engine result against the same value
/// before the method returns it to the editor.
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

    /// Build engine input from the target and every applicable sibling in this editor snapshot.
    pub(crate) fn target_document(&self) -> Result<DocumentAnalysisSnapshot, Error> {
        self.captured
            .analysis_snapshot(&self.engine_client)
            .map_err(|unavailable| {
                tracing::debug!(
                    path = ?unavailable.path().map(std::path::Path::display),
                    reason = unavailable.reason(),
                    "editor snapshot is temporarily unavailable"
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

    /// Return a document query only if its tagged input still matches live editor state.
    ///
    /// Ordinary methods cannot retry inside the same handler. If the editor changed while the
    /// engine was working, return `ContentModified` and let the LSP client decide whether to ask
    /// again.
    pub(crate) fn finish_query<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<T>>,
    ) -> Result<T, Error> {
        analysis_result::document_query(result, &self.captured)?.into_lsp_result()
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

    /// Check one completed engine attempt without deciding whether the LSP request should end.
    ///
    /// `EditorChanged` goes back to the completion loop, which can take a newer snapshot and try
    /// again. Engine aborts, transport failures, and mismatched response tags are still errors.
    pub(crate) fn finish_attempt<T>(
        &self,
        result: anyhow::Result<AnalysisOutcome<T>>,
    ) -> Result<DocumentQueryStatus<T>, Error> {
        analysis_result::document_query(result, self.document.captured_document())
    }
}
