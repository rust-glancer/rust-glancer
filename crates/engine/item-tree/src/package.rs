use rg_arena::Arena;
use rg_parse::{FileId, TargetId};

use crate::{Documentation, ItemNode, ItemTreeId, ItemTreeRef};

/// Item trees for all files inside one parsed package, plus target entrypoints.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    pub(crate) files: Arena<FileId, Option<FileTree>>,
    pub(crate) target_roots: Arena<TargetId, TargetRoot>,
}

impl Package {
    /// Returns all file trees.
    pub fn files(&self) -> impl Iterator<Item = &FileTree> {
        self.files.iter().filter_map(Option::as_ref)
    }

    /// Returns one file tree by parsed file id.
    pub fn file(&self, file_id: FileId) -> Option<&FileTree> {
        self.files.get(file_id)?.as_ref()
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
    pub fn target_root(&self, target_id: TargetId) -> Option<&TargetRoot> {
        self.target_roots.get(target_id)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        for file in self.files.iter_mut().flatten() {
            file.shrink_to_fit();
        }
        self.files.shrink_to_fit();
        self.target_roots.shrink_to_fit();
    }
}

/// File-local lowered item tree.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    fn shrink_to_fit(&mut self) {
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.top_level.shrink_to_fit();
        for item in self.items.iter_mut() {
            item.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}

/// Target entrypoint into file-local item trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRoot {
    pub target: TargetId,
    pub root_file: FileId,
}
