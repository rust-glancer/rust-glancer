use std::path::PathBuf;

use rg_project::PackageResidencyPolicy;
use rg_workspace::CargoMetadataConfig;
use tokio::sync::oneshot;

pub(super) type EngineResponse<T> = oneshot::Sender<anyhow::Result<T>>;

#[derive(Debug)]
pub(super) enum EngineCommand {
    Initialize {
        root: PathBuf,
        package_residency_policy: PackageResidencyPolicy,
        cargo_metadata_config: CargoMetadataConfig,
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
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    GotoTypeDefinition {
        path: PathBuf,
        position: ls_types::Position,
        respond_to: EngineResponse<Vec<ls_types::Location>>,
    },
    Hover {
        path: PathBuf,
        position: ls_types::Position,
        respond_to: EngineResponse<Option<ls_types::Hover>>,
    },
    Completion {
        path: PathBuf,
        position: ls_types::Position,
        respond_to: EngineResponse<Vec<ls_types::CompletionItem>>,
    },
    DocumentSymbol {
        path: PathBuf,
        respond_to: EngineResponse<Vec<ls_types::DocumentSymbol>>,
    },
    InlayHint {
        path: PathBuf,
        range: ls_types::Range,
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
