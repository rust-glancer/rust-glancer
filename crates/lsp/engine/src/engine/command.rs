//! Messages crossing from async RPC tasks onto the synchronous analysis lane.
//!
//! Editor requests carry a one-shot response endpoint with the command. Deferred indexing is the
//! exception: its background thread sends an internal completion command back through the same
//! queue, so project generation checks still happen in FIFO order with every other mutation.

use std::path::PathBuf;

use rg_lsp_proto::{
    AnalysisOutcome, CompletionClientCapabilities, CompletionResult, DocumentPositionSnapshot,
    DocumentQueryResult, DocumentRangeSnapshot, EditorDocumentSnapshot, GlobalOperationResult,
    GlobalPositionSnapshot,
};
use rg_project::SavedFileChange;
use tokio::sync::oneshot;

use super::ProjectConfiguration;

/// Response endpoint owned by one request until the engine dispatcher answers it.
pub(crate) type EngineResponse<T> = oneshot::Sender<anyhow::Result<T>>;
/// Response endpoint for a semantic request that may abort without producing a feature value.
pub(crate) type AnalysisResponse<T> = EngineResponse<AnalysisOutcome<T>>;
/// Result returned by the detached deferred-indexing thread to the project coordinator.
pub(crate) type DeferredIndexingResult = anyhow::Result<Box<rg_project::FinishedSplitIndexing>>;

/// Work accepted by the one analysis thread.
///
/// Keeping project mutations and analysis queries in one enum makes their ordering explicit. RPC
/// handlers may run concurrently, but the dispatcher consumes these commands one at a time.
#[derive(Debug)]
pub(crate) enum EngineCommand {
    Initialize {
        root: PathBuf,
        configuration: ProjectConfiguration,
        respond_to: EngineResponse<()>,
    },
    /// Background repair scheduled when a query proves that saved analysis is stale.
    RecoverStaleSource {
        path: PathBuf,
    },
    /// Exact captured sources, optionally paired with graph/discovery path changes.
    SavedProjectChanges {
        changes: Vec<SavedFileChange>,
        respond_to: EngineResponse<u64>,
    },
    GotoDefinition {
        input: GlobalPositionSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Vec<ls_types::Location>>>,
    },
    GotoTypeDefinition {
        input: GlobalPositionSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Vec<ls_types::Location>>>,
    },
    GotoImplementation {
        input: GlobalPositionSnapshot,
        respond_to: AnalysisResponse<GlobalOperationResult<Vec<ls_types::Location>>>,
    },
    References {
        input: GlobalPositionSnapshot,
        include_declaration: bool,
        respond_to: AnalysisResponse<GlobalOperationResult<Vec<ls_types::Location>>>,
    },
    PrepareRename {
        input: GlobalPositionSnapshot,
        respond_to:
            AnalysisResponse<GlobalOperationResult<Option<ls_types::PrepareRenameResponse>>>,
    },
    Rename {
        input: GlobalPositionSnapshot,
        new_name: String,
        respond_to: AnalysisResponse<GlobalOperationResult<Option<ls_types::WorkspaceEdit>>>,
    },
    DocumentHighlight {
        input: DocumentPositionSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Vec<ls_types::DocumentHighlight>>>,
    },
    Hover {
        input: DocumentPositionSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Option<ls_types::Hover>>>,
    },
    Completion {
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
        respond_to: AnalysisResponse<CompletionResult>,
    },
    Formatting {
        snapshot: EditorDocumentSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Option<Vec<ls_types::TextEdit>>>>,
    },
    DocumentSymbol {
        snapshot: EditorDocumentSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Vec<ls_types::DocumentSymbol>>>,
    },
    InlayHint {
        input: DocumentRangeSnapshot,
        respond_to: AnalysisResponse<DocumentQueryResult<Vec<ls_types::InlayHint>>>,
    },
    WorkspaceSymbol {
        query: String,
        respond_to: AnalysisResponse<Vec<ls_types::WorkspaceSymbol>>,
    },
    ReindexWorkspace {
        respond_to: EngineResponse<()>,
    },
    /// Re-enters a background result onto the lane that owns the saved project.
    DeferredIndexingFinished {
        generation: u64,
        result: DeferredIndexingResult,
    },
    Shutdown(EngineResponse<()>),
}
