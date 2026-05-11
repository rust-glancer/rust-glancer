use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_lsp_proto::{AnalysisConfig, DiagnosticsConfig};
use tokio::sync::Notify;

use super::{
    routing::{EngineId, EngineRouting, WorkspaceEngineRoute},
    slot::EngineSlot,
};

/// Mutable registry state guarded by `EngineRegistry`'s mutex.
///
/// The key invariant here is that routing reservations and engine slot allocation happen together:
/// once routing hands out a fresh `EngineId`, the same lock scope pushes the corresponding slot.
#[derive(Debug)]
pub(super) struct EngineRegistryInner {
    pub(super) routing: EngineRouting,
    pub(super) engines: Vec<EngineSlot>,
    analysis_config: AnalysisConfig,
    diagnostics_config: DiagnosticsConfig,
    last_published_workspace: Option<PathBuf>,
}

impl EngineRegistryInner {
    pub(super) fn new(
        workspace_folders: impl IntoIterator<Item = PathBuf>,
        analysis_config: AnalysisConfig,
        diagnostics_config: DiagnosticsConfig,
    ) -> Self {
        let mut routing = EngineRouting::default();
        routing.set_workspace_folders(workspace_folders);

        Self {
            routing,
            engines: Vec::new(),
            analysis_config,
            diagnostics_config,
            last_published_workspace: None,
        }
    }

    pub(super) fn open_file_owner(&self, path: &Path) -> Option<EngineId> {
        self.routing.open_file_owner(path)
    }

    pub(super) fn set_open_file(&mut self, path: PathBuf, id: EngineId) {
        self.routing.set_open_file(path, id);
    }

    pub(super) fn remove_open_file(
        &mut self,
        path: &Path,
        owner: Option<EngineId>,
    ) -> Option<EngineId> {
        self.routing.remove_open_file(path, owner)
    }

    pub(super) fn reserve_workspace_root(
        &mut self,
        workspace_root: PathBuf,
    ) -> Option<ReservedEngineRoute> {
        let route = self.routing.route_workspace_root(workspace_root)?;
        Some(self.reserve_workspace_route(route))
    }

    pub(super) fn engine(&self, id: EngineId) -> Option<&EngineSlot> {
        self.engines.get(id.index())
    }

    pub(super) fn active_ready_id(&self) -> Option<EngineId> {
        let id = self.routing.active_id()?;
        self.engine(id).and_then(EngineSlot::ready)?;
        Some(id)
    }

    pub(super) fn set_active_id(&mut self, id: EngineId) {
        self.routing.set_active_id(id);
    }

    /// Returns the active-workspace display root if the client has not seen it yet.
    pub(super) fn active_workspace_to_publish(&mut self, id: EngineId) -> Option<PathBuf> {
        let root = self
            .routing
            .root_for_id(id)
            .map(Path::to_path_buf)
            .expect("ready engine id should have a routing root");

        if self.last_published_workspace.as_deref() == Some(root.as_path()) {
            return None;
        }

        self.last_published_workspace = Some(root.clone());
        Some(root)
    }

    fn reserve_workspace_route(&mut self, route: WorkspaceEngineRoute) -> ReservedEngineRoute {
        match route {
            WorkspaceEngineRoute::Existing(id) => ReservedEngineRoute::Existing(id),
            WorkspaceEngineRoute::Spawn { new_id, root } => {
                let config = self.spawn_config();

                // The slot is visible before the process exists. Concurrent routes for the same
                // root will now receive `Existing(new_id)` and wait on this notification.
                self.push_slot(
                    new_id,
                    EngineSlot::Starting {
                        notify: Arc::new(Notify::new()),
                    },
                );

                ReservedEngineRoute::Spawn(ReservedEngineStart {
                    id: new_id,
                    root,
                    config,
                })
            }
        }
    }

    fn spawn_config(&self) -> EngineSpawnConfig {
        EngineSpawnConfig {
            analysis: self.analysis_config.clone(),
            diagnostics: self.diagnostics_config.clone(),
        }
    }

    fn push_slot(&mut self, id: EngineId, slot: EngineSlot) {
        assert_eq!(
            id.index(),
            self.engines.len(),
            "reserved engine id should match next engine slot"
        );
        self.engines.push(slot);
    }
}

#[derive(Debug)]
pub(super) struct EngineSpawnConfig {
    pub(super) analysis: AnalysisConfig,
    pub(super) diagnostics: DiagnosticsConfig,
}

/// Registry action after routing has been resolved under the state lock.
#[derive(Debug)]
pub(super) enum ReservedEngineRoute {
    Existing(EngineId),
    Spawn(ReservedEngineStart),
}

impl ReservedEngineRoute {
    pub(super) fn id(&self) -> EngineId {
        match self {
            Self::Existing(id) => *id,
            Self::Spawn(start) => start.id,
        }
    }
}

/// Token proving that an engine id has been reserved and needs process startup.
#[derive(Debug)]
pub(super) struct ReservedEngineStart {
    pub(super) id: EngineId,
    pub(super) root: PathBuf,
    pub(super) config: EngineSpawnConfig,
}
