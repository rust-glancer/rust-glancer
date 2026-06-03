use std::path::PathBuf;

use rg_lsp_proto::{AnalysisConfig, CompletionClientCapabilities};
use tokio::sync::oneshot;

use crate::documents::DirtyDocumentSnapshot;

pub(crate) type EngineResponse<T> = oneshot::Sender<anyhow::Result<T>>;

#[derive(Debug)]
pub(crate) enum EngineCommand {
    Initialize {
        root: PathBuf,
        analysis: AnalysisConfig,
        respond_to: EngineResponse<()>,
    },
    DidSave {
        path: PathBuf,
        text: Option<String>,
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
    Shutdown(EngineResponse<()>),
}
