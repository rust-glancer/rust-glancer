//! Messages crossing from async RPC tasks onto the synchronous analysis lane.
//!
//! Editor requests carry a one-shot response endpoint with the command. Deferred indexing is the
//! exception: its background thread sends an internal completion command back through the same
//! queue, so project generation checks still happen in FIFO order with every other mutation.

use std::{path::PathBuf, sync::Arc};

use rg_lsp_proto::CompletionClientCapabilities;
use tokio::sync::oneshot;

use crate::documents::DirtyDocumentSnapshot;

use super::ProjectConfiguration;

/// Response endpoint owned by one request until the engine dispatcher answers it.
pub(crate) type EngineResponse<T> = oneshot::Sender<anyhow::Result<T>>;
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
    ProjectPathsChanged {
        paths: Vec<PathBuf>,
        respond_to: EngineResponse<()>,
    },
    GotoDefinition {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    GotoTypeDefinition {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    GotoImplementation {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    References {
        path: PathBuf,
        position: ls_types::Position,
        include_declaration: bool,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    PrepareRename {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Option<ls_types::PrepareRenameResponse>>,
    },
    Rename {
        path: PathBuf,
        position: ls_types::Position,
        new_name: String,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Option<ls_types::WorkspaceEdit>>,
    },
    DocumentHighlight {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::DocumentHighlight>>,
    },
    Hover {
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Option<ls_types::Hover>>,
    },
    Completion {
        path: PathBuf,
        position: ls_types::Position,
        client_capabilities: CompletionClientCapabilities,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::CompletionItem>>,
    },
    Formatting {
        path: PathBuf,
        text: Arc<str>,
        respond_to: EngineResponse<Vec<ls_types::TextEdit>>,
    },
    DocumentSymbol {
        path: PathBuf,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::DocumentSymbol>>,
    },
    InlayHint {
        path: PathBuf,
        range: ls_types::Range,
        dirty: Option<DirtyDocumentSnapshot>,
        respond_to: EngineResponse<Vec<ls_types::InlayHint>>,
    },
    WorkspaceSymbol {
        query: String,
        respond_to: EngineResponse<Vec<ls_types::WorkspaceSymbol>>,
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
