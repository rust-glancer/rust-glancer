use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use rg_arena::Arena;
use rg_ir_model::FileId;
use rg_parse::CargoTargetId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{Documentation, IncludePathExpression, ItemNode, ItemTreeId, ItemTreeRef};

/// Item trees for all files inside one parsed package, plus target entrypoints.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct Package {
    pub(crate) files: BTreeMap<FileId, Arc<FileTree>>,
    pub(crate) includes: HashMap<FileId, HashMap<IncludePathExpression, FileId>>,
    pub(crate) target_roots: Arena<CargoTargetId, TargetRoot>,
}

impl Package {
    /// Returns all file trees.
    pub fn files(&self) -> impl Iterator<Item = &FileTree> {
        self.files.values().map(Arc::as_ref)
    }

    /// Returns one file tree by parsed file id.
    pub fn file(&self, file_id: FileId) -> Option<&FileTree> {
        self.files.get(&file_id).map(Arc::as_ref)
    }

    /// Routing captured before DefMap checks whether the call actually names the include builtin.
    pub fn included_file(&self, origin: FileId, path: &IncludePathExpression) -> Option<FileId> {
        self.includes.get(&origin)?.get(path).copied()
    }

    /// Returns one lowered item by stable item-tree reference.
    pub fn item(&self, item_ref: ItemTreeRef) -> Option<&ItemNode> {
        self.file(item_ref.file_id)?.item(item_ref.item)
    }

    /// Returns all target roots.
    pub fn target_roots(&self) -> &[TargetRoot] {
        self.target_roots.as_slice()
    }

    /// Returns one target root by parsed target id.
    pub fn target_root(&self, target_id: CargoTargetId) -> Option<&TargetRoot> {
        self.target_roots.get(target_id)
    }
}

/// File-local lowered item tree.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink, SchemaRead, SchemaWrite)]
pub struct FileTree {
    pub file: FileId,
    pub docs: Option<Documentation>,
    pub top_level: Vec<ItemTreeId>,
    pub items: Arena<ItemTreeId, ItemNode>,
}

impl FileTree {
    /// Returns one file-local item-tree node by id.
    pub fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }
}

/// Target entrypoint into file-local item trees.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct TargetRoot {
    pub target: CargoTargetId,
    pub root_file: FileId,
}
