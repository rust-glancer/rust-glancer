use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Pure routing state for choosing which engine owns an LSP operation.
///
/// The registry owns engine slots and RPC clients; this table owns path knowledge: workspace
/// folders, Cargo roots, and which stable engine id belongs to each root.
#[derive(Debug, Default)]
pub(crate) struct EngineRouting {
    workspace_folders: Vec<PathBuf>,
    engine_ids_by_root: BTreeMap<PathBuf, EngineId>,
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

    /// Routes an explicit Cargo root, reserving a fresh engine id when needed.
    ///
    /// `Spawn` is a reservation: after this method returns it, the root already maps to `new_id`.
    /// Concurrent callers therefore converge on the same id while the registry starts the process.
    pub(crate) fn route_root(&mut self, root: PathBuf) -> DocumentEngineRoute {
        let root = normalize_path(root);
        if let Some(id) = self.engine_ids_by_root.get(&root).copied() {
            return DocumentEngineRoute::Existing(id);
        }

        let new_id = EngineId(self.engine_ids_by_root.len());
        self.engine_ids_by_root.insert(root.clone(), new_id);
        DocumentEngineRoute::Spawn { new_id, root }
    }

    /// Routes a document path to an existing engine, a new workspace engine, or the active engine.
    pub(crate) fn route_document(&mut self, path: &Path) -> Option<DocumentEngineRoute> {
        let path = normalize_path(path);

        if let Some(id) = self.engine_id_for_path(&path) {
            return Some(DocumentEngineRoute::Existing(id));
        }

        if self.is_in_workspace(&path) {
            return nearest_cargo_root(&path).map(|root| self.route_root(root));
        }

        self.active_id().map(DocumentEngineRoute::Existing)
    }

    #[cfg(test)]
    pub(crate) fn root_for_id(&self, id: EngineId) -> Option<&Path> {
        self.engine_ids_by_root
            .iter()
            .find_map(|(root, candidate)| (*candidate == id).then_some(root.as_path()))
    }

    fn engine_id_for_path(&self, path: &Path) -> Option<EngineId> {
        self.engine_ids_by_root
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, id)| *id)
    }

    fn is_in_workspace(&self, path: &Path) -> bool {
        self.workspace_folders
            .iter()
            .any(|workspace_folder| path.starts_with(workspace_folder))
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

/// Routing decision before the registry has materialized any new process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DocumentEngineRoute {
    Existing(EngineId),
    Spawn { new_id: EngineId, root: PathBuf },
}

pub(crate) fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn nearest_cargo_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.is_dir().then_some(path).or_else(|| path.parent());
    while let Some(candidate) = current {
        if candidate.join("Cargo.toml").is_file() {
            return Some(normalize_path(candidate));
        }
        current = candidate.parent();
    }
    None
}
