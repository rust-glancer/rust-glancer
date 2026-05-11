use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Pure routing state for choosing which engine owns an LSP operation.
///
/// The registry owns engine slots and RPC clients; this table owns path knowledge: workspace
/// folders, Cargo workspace roots, and exact open-file ownership.
#[derive(Debug, Default)]
pub(crate) struct EngineRouting {
    workspace_folders: Vec<PathBuf>,
    engine_ids_by_root: BTreeMap<PathBuf, EngineId>,
    engine_ids_by_open_file: BTreeMap<PathBuf, EngineId>,
    last_active_id: Option<EngineId>,
}

impl EngineRouting {
    /// Replaces the VS Code workspace folders that are allowed to auto-spawn engines.
    pub(crate) fn set_workspace_folders(&mut self, folders: impl IntoIterator<Item = PathBuf>) {
        self.workspace_folders = folders.into_iter().map(normalize_path).collect();
        self.workspace_folders.sort();
        self.workspace_folders.dedup();
    }

    pub(crate) fn set_active_id(&mut self, id: EngineId) {
        self.last_active_id = Some(id);
    }

    pub(crate) fn active_id(&self) -> Option<EngineId> {
        self.last_active_id
    }

    /// Returns whether a document path is inside a folder where engines may be discovered.
    pub(crate) fn can_discover_workspace_for(&self, path: &Path) -> bool {
        let path = normalize_path(path);
        self.workspace_folders
            .iter()
            .any(|workspace_folder| path.starts_with(workspace_folder.as_path()))
    }

    /// Routes a Cargo-resolved workspace root into its owning workspace engine.
    ///
    /// `Spawn` is a reservation: after this method returns it, the root already maps to `new_id`.
    /// Concurrent callers therefore converge on the same id while the registry starts the process.
    pub(crate) fn route_workspace_root(
        &mut self,
        workspace_root: PathBuf,
    ) -> Option<WorkspaceEngineRoute> {
        let workspace_root = normalize_path(workspace_root);
        if let Some(id) = self.engine_ids_by_root.get(&workspace_root).copied() {
            return Some(WorkspaceEngineRoute::Existing(id));
        }

        if !self.is_workspace_root_allowed(&workspace_root) {
            return None;
        }

        let new_id = EngineId(self.engine_ids_by_root.len());
        self.engine_ids_by_root
            .insert(workspace_root.clone(), new_id);
        Some(WorkspaceEngineRoute::Spawn {
            new_id,
            root: workspace_root,
        })
    }

    /// Records which engine owns an opened document.
    pub(crate) fn set_open_file(&mut self, path: PathBuf, id: EngineId) {
        self.engine_ids_by_open_file
            .insert(normalize_path(path), id);
    }

    /// Removes ownership unconditionally, or only when it still belongs to the expected engine.
    pub(crate) fn remove_open_file(
        &mut self,
        path: &Path,
        owner: Option<EngineId>,
    ) -> Option<EngineId> {
        let path = normalize_path(path);
        if owner.is_none() || self.engine_ids_by_open_file.get(&path).copied() == owner {
            return self.engine_ids_by_open_file.remove(&path);
        }

        None
    }

    /// Returns the engine that owns an open document, if the editor told us about it.
    pub(crate) fn open_file_owner(&self, path: &Path) -> Option<EngineId> {
        self.engine_ids_by_open_file
            .get(&normalize_path(path))
            .copied()
    }

    pub(crate) fn root_for_id(&self, id: EngineId) -> Option<&Path> {
        self.engine_ids_by_root
            .iter()
            .find_map(|(root, candidate)| (*candidate == id).then_some(root.as_path()))
    }

    fn is_workspace_root_allowed(&self, root: &Path) -> bool {
        self.workspace_folders
            .iter()
            .any(|workspace_folder| root.starts_with(workspace_folder.as_path()))
    }
}

/// Stable engine identity allocated by routing and used as an index into registry slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EngineId(usize);

impl EngineId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// Routing decision for a known Cargo workspace root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceEngineRoute {
    Existing(EngineId),
    Spawn { new_id: EngineId, root: PathBuf },
}

pub(crate) fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
