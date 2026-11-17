use std::{collections::BTreeMap, sync::Arc};

use rg_arena::Arena;
use rg_ir_model::FileId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{Documentation, ItemNode, ItemTreeId, ItemTreeRef};

/// File trees acquired while constructing a target's declarations.
#[derive(Debug, Default)]
pub struct ItemTrees {
    files: BTreeMap<FileId, Arc<FileTree>>,
}

impl ItemTrees {
    /// Add file trees acquired for one target's declaration build.
    pub fn extend_files(&mut self, trees: impl IntoIterator<Item = Arc<FileTree>>) {
        self.files
            .extend(trees.into_iter().map(|tree| (tree.file, tree)));
    }

    /// Returns all file trees.
    pub fn files(&self) -> impl Iterator<Item = &FileTree> {
        self.files.values().map(Arc::as_ref)
    }

    /// Returns one file tree by parsed file id.
    pub fn file(&self, file_id: FileId) -> Option<&FileTree> {
        self.files.get(&file_id).map(Arc::as_ref)
    }

    /// Returns one lowered item by stable item-tree reference.
    pub fn item(&self, item_ref: ItemTreeRef) -> Option<&ItemNode> {
        self.file(item_ref.file_id)?.item(item_ref.item)
    }
}

/// File-local lowered item tree.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink, SchemaRead, SchemaWrite)]
pub struct FileTree {
    pub file: FileId,
    pub docs: Option<Documentation>,
    pub top_level: Vec<ItemTreeId>,
    pub items: Arena<ItemTreeId, ItemNode>,
    /// Whether declaration building must also depend on the file's exact source text.
    ///
    /// An unchanged item tree normally lets DefMap reuse its result after whitespace or body edits.
    /// This relies on lowering retaining all the declaration facts needed to decide whether reuse
    /// is safe. Parser recovery outside a closed function body or terminated initializer, or
    /// assembly syntax whose payload is omitted here, makes that assumption too strong.
    ///
    /// When this is true, DefMap also reads `SourceInput`. Any text change then reruns its build,
    /// even if the lowered items compare equal. This only controls reuse; it does not change how
    /// source IDs or ranges are interpreted.
    pub source_sensitive: bool,
}

impl FileTree {
    /// Returns one file-local item-tree node by id.
    pub fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }
}
