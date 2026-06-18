use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::sync::Notify;

use crate::{
    client_notifications::{ActiveWorkspaceState, ActiveWorkspaceStatus},
    config::ServerConfig,
};

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
    config: ServerConfig,
    last_published_workspace_status: Option<ActiveWorkspaceStatus>,
    shutting_down: bool,
}

impl EngineRegistryInner {
    pub(super) fn new(
        workspace_folders: impl IntoIterator<Item = PathBuf>,
        config: ServerConfig,
    ) -> Self {
        let mut routing = EngineRouting::default();
        routing.set_workspace_folders(workspace_folders);

        Self {
            routing,
            engines: Vec::new(),
            config,
            last_published_workspace_status: None,
            shutting_down: false,
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

    pub(super) fn begin_shutdown(&mut self) {
        self.shutting_down = true;
    }

    pub(super) fn shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// Returns the active-workspace status if the client has not seen this exact state yet.
    pub(super) fn workspace_status_update(&mut self) -> Option<ActiveWorkspaceStatus> {
        let id = self.routing.active_id()?;
        let status = self.workspace_status(id);

        if self.last_published_workspace_status.as_ref() == Some(&status) {
            return None;
        }

        self.last_published_workspace_status = Some(status.clone());
        Some(status)
    }

    fn workspace_status(&self, id: EngineId) -> ActiveWorkspaceStatus {
        let root = self
            .routing
            .root_for_id(id)
            .map(Path::to_path_buf)
            .expect("engine id should have a routing root");
        let slot = self.engine(id).expect("engine id should have a slot");
        let (state, message) = match slot {
            EngineSlot::Starting { .. } => (ActiveWorkspaceState::Indexing, None),
            EngineSlot::Ready(_) => (ActiveWorkspaceState::Ready, None),
            EngineSlot::Failed { error, .. } => {
                (ActiveWorkspaceState::Failed, Some(error.to_string()))
            }
        };

        ActiveWorkspaceStatus {
            root,
            state,
            message,
        }
    }

    fn reserve_workspace_route(&mut self, route: WorkspaceEngineRoute) -> ReservedEngineRoute {
        match route {
            WorkspaceEngineRoute::Existing(id) => ReservedEngineRoute::Existing(id),
            WorkspaceEngineRoute::Spawn { new_id, root } => {
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
                    config: self.config.engine_config_for_root(&root),
                    root,
                })
            }
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
    pub(super) config: rg_lsp_proto::EngineConfig,
}

#[cfg(test)]
mod tests {
    use rg_lsp_proto::CargoMetadataTarget;
    use serde_json::json;

    use super::*;

    #[test]
    fn reserved_spawn_uses_config_for_exact_workspace_root() {
        let options = json!({
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
                "features": ["base"],
                "overrides": [{
                    "path": "project-a",
                    "target": null,
                    "features": ["override"],
                }],
            },
        });
        let config =
            ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
                .expect("server config should parse");
        let mut inner = EngineRegistryInner::new([PathBuf::from("/repo")], config);

        let route = inner
            .reserve_workspace_root(PathBuf::from("/repo/project-a"))
            .expect("configured root should reserve an engine");
        let ReservedEngineRoute::Spawn(start) = route else {
            panic!("first route for workspace root should spawn");
        };

        assert_eq!(
            start.config.analysis.cargo_metadata_config.target(),
            &CargoMetadataTarget::Auto,
        );
        assert_eq!(
            start.config.analysis.cargo_metadata_config.features(),
            &["override".to_string()],
        );
    }
}
