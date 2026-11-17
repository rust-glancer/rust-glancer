//! Request data passed from `Backend` to document-scoped method handlers.
//!
//! Every document handler starts with editor state captured by ingress before the async handler
//! runs. Semantic reads send all applicable documents to the engine; pure syntax reads need only
//! their target. Publication checks use the same captured scope as analysis.
//!
//! Completion has a second context for its scheduler's logical request and client capabilities.
//! The document context still describes one attempt. Completion and inlay queries can replace
//! that capture before retrying after an edit.

use std::path::Path;

use rg_lsp_proto::{
    CompletionClientCapabilities, DocumentPositionSnapshot, DocumentRangeSnapshot,
    DocumentSnapshot, EditorDocumentSnapshot, QueryError, QueryValue,
};
use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Error};

use super::query_response::{self, into_lsp_error, temporarily_unavailable};
use crate::{
    completion_scheduler::CompletionRequest, engine_client::EngineClient, ingress::CapturedDocument,
};

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

    /// Replace the snapshot used by the next engine attempt.
    pub(crate) fn replace_document(&mut self, captured: CapturedDocument) {
        self.captured = captured;
    }

    /// Build engine input containing only the captured target document.
    pub(crate) fn target_document(&self) -> Result<EditorDocumentSnapshot, Error> {
        self.captured
            .target_document(&self.engine_client)
            .map_err(|unavailable| {
                tracing::debug!(
                    path = ?unavailable.path().map(Path::display),
                    reason = unavailable.reason(),
                    "exact target document input is temporarily unavailable"
                );
                temporarily_unavailable(unavailable.reason())
            })
    }

    pub(crate) fn semantic_document(&self) -> Result<DocumentSnapshot, Error> {
        self.captured
            .semantic_document(&self.engine_client)
            .map_err(|unavailable| temporarily_unavailable(unavailable.reason()))
    }

    pub(crate) fn position(&self, position: Position) -> Result<DocumentPositionSnapshot, Error> {
        Ok(self.semantic_document()?.with_position(position))
    }

    pub(crate) fn range(&self, range: Range) -> Result<DocumentRangeSnapshot, Error> {
        Ok(self.semantic_document()?.with_range(range))
    }

    pub(crate) fn finish_semantic_query<T>(
        &self,
        result: Result<QueryValue<T>, QueryError>,
    ) -> Result<T, Error> {
        self.finish_attempt(result).map_err(into_lsp_error)
    }

    pub(crate) fn finish_syntax_query<T>(
        &self,
        result: Result<QueryValue<T>, QueryError>,
    ) -> Result<T, Error> {
        query_response::validate_target_document(result, &self.captured).map_err(into_lsp_error)
    }

    /// Check one completed engine attempt without deciding whether the LSP request should end.
    ///
    /// `EditorChanged` goes back to the handler's retry loop, which can take a newer snapshot and
    /// try again. Other query failures and mismatched response scopes are returned to the caller.
    pub(crate) fn finish_attempt<T>(
        &self,
        result: Result<QueryValue<T>, QueryError>,
    ) -> Result<T, QueryError> {
        query_response::validate_document(result, &self.captured)
    }
}

/// Document context plus the state used only by the completion retry loop.
///
/// Completion submits its engine queries through one logical request owned by the scheduler and
/// passes the client's completion capabilities to the engine. This type makes that extra state
/// mandatory for completion without exposing it to every other handler.
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

    /// Select the completion position within the complete editor capture.
    pub(crate) fn input(&self, position: Position) -> Result<DocumentPositionSnapshot, Error> {
        self.document.position(position)
    }
}
