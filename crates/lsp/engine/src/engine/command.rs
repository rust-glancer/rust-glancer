//! Messages crossing from async RPC tasks onto the synchronous analysis lane.
//!
//! Editor requests carry a one-shot response endpoint with the command. Deferred indexing is the
//! exception: its background thread sends internal progress and completion commands back through
//! the same queue, so project generation checks still happen in FIFO order with every other
//! mutation.

use std::path::PathBuf;

use rg_lsp_proto::{
    CodeActionRequestContext, CompletionClientCapabilities, DocumentPositionSnapshot,
    DocumentRangeSnapshot, EditorDocumentSnapshot, FoldingClientCapabilities,
    GlobalPositionSnapshot, QueryError, QueryValue,
};
use rg_project::{SavedBodyProducts, SavedFileChange, SplitIndexingProgress};
use tokio::sync::oneshot;

use super::ProjectConfiguration;

/// Response endpoint owned by one request until the engine dispatcher answers it.
pub(crate) type EngineResponder<T> = oneshot::Sender<anyhow::Result<T>>;
/// Response endpoint for a semantic request that may finish without a publishable feature value.
pub(crate) type QueryResponder<T> = oneshot::Sender<Result<QueryValue<T>, QueryError>>;
/// Completion status after the body worker's jobs have drained. Products arrive in separate commands.
pub(crate) type DeferredIndexingResult = anyhow::Result<()>;

/// Work accepted by the one analysis thread.
///
/// Keeping project mutations and analysis queries in one enum makes their ordering explicit. RPC
/// handlers may run concurrently, but the dispatcher consumes these commands one at a time.
#[derive(Debug)]
pub(crate) enum EngineCommand {
    Initialize {
        root: PathBuf,
        configuration: ProjectConfiguration,
        respond_to: EngineResponder<()>,
    },
    /// Background repair scheduled when a query proves that saved analysis is stale.
    RecoverStaleSource {
        path: PathBuf,
    },
    /// Exact captured sources, optionally paired with graph/discovery path changes.
    SavedProjectChanges {
        changes: Vec<SavedFileChange>,
        respond_to: EngineResponder<u64>,
    },
    GotoDefinition {
        input: GlobalPositionSnapshot,
        respond_to: QueryResponder<Vec<ls_types::Location>>,
    },
    GotoTypeDefinition {
        input: GlobalPositionSnapshot,
        respond_to: QueryResponder<Vec<ls_types::Location>>,
    },
    GotoImplementation {
        input: GlobalPositionSnapshot,
        respond_to: QueryResponder<Vec<ls_types::Location>>,
    },
    References {
        input: GlobalPositionSnapshot,
        include_declaration: bool,
        respond_to: QueryResponder<Vec<ls_types::Location>>,
    },
    PrepareRename {
        input: GlobalPositionSnapshot,
        respond_to: QueryResponder<Option<ls_types::PrepareRenameResponse>>,
    },
    Rename {
        input: GlobalPositionSnapshot,
        new_name: String,
        respond_to: QueryResponder<Option<ls_types::WorkspaceEdit>>,
    },
    DocumentHighlight {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Vec<ls_types::DocumentHighlight>>,
    },
    Hover {
        input: GlobalPositionSnapshot,
        respond_to: QueryResponder<Option<ls_types::Hover>>,
    },
    CodeAction {
        input: DocumentRangeSnapshot,
        request_context: CodeActionRequestContext,
        respond_to: QueryResponder<Vec<ls_types::CodeAction>>,
    },
    Completion {
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
        respond_to: QueryResponder<Vec<ls_types::CompletionItem>>,
    },
    Formatting {
        snapshot: EditorDocumentSnapshot,
        respond_to: QueryResponder<Option<Vec<ls_types::TextEdit>>>,
    },
    DocumentSymbol {
        snapshot: EditorDocumentSnapshot,
        respond_to: QueryResponder<Vec<ls_types::DocumentSymbol>>,
    },
    FoldingRange {
        snapshot: EditorDocumentSnapshot,
        client_capabilities: FoldingClientCapabilities,
        respond_to: QueryResponder<Vec<ls_types::FoldingRange>>,
    },
    InlayHint {
        input: DocumentRangeSnapshot,
        respond_to: QueryResponder<Vec<ls_types::InlayHint>>,
    },
    WorkspaceSymbol {
        query: String,
        respond_to: QueryResponder<Vec<ls_types::WorkspaceSymbol>>,
    },
    ReindexWorkspace {
        respond_to: EngineResponder<()>,
    },
    /// Update the editor-derived package priority used by deferred background indexing.
    SetDeferredIndexingPriority {
        path: PathBuf,
        prioritized: bool,
        respond_to: EngineResponder<()>,
    },
    /// Transfer completed body products while construction continues in the background.
    DeferredIndexingProducts {
        products: Box<SavedBodyProducts>,
    },
    /// Publish one coalesced progress snapshot from the body worker.
    DeferredIndexingProgress {
        generation: u64,
        progress: SplitIndexingProgress,
    },
    /// End the worker lifecycle after every completed product batch has been enqueued.
    DeferredIndexingFinished {
        generation: u64,
        result: DeferredIndexingResult,
    },
    Shutdown(EngineResponder<()>),
}
