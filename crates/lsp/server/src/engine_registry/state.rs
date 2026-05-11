use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_lsp_proto::EngineConfig;
use tokio::sync::Notify;

use crate::client_notifications::{ActiveWorkspaceState, ActiveWorkspaceStatus};

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
    config: EngineConfig,
    last_published_workspace_status: Option<ActiveWorkspaceStatus>,
}

impl EngineRegistryInner {
    pub(super) fn new(
        workspace_folders: impl IntoIterator<Item = PathBuf>,
        config: EngineConfig,
    ) -> Self {
        let mut routing = EngineRouting::default();
        routing.set_workspace_folders(workspace_folders);

        Self {
            routing,
            engines: Vec::new(),
            config,
            last_published_workspace_status: None,
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
                    root,
                    config: self.config.clone(),
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
    pub(super) config: EngineConfig,
}
