use crate::items::{ItemNode, ItemTreeId, ItemTreeRef};
use rg_arena::Arena;
use rg_parse::{FileId, Span};
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::BodyRef;

/// Stable identifier of one retained macro expansion payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub struct GeneratedSourceId(pub usize);

impl rg_arena::ArenaId for GeneratedSourceId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Target-local reference to one generated item payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct GeneratedItemRef {
    pub source: GeneratedSourceId,
    pub item: ItemTreeId,
}

/// Body-local reference to one item-tree-shaped source payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyItemSourceRef {
    pub body: BodyRef,
    pub item: ItemTreeId,
}

/// Durable source identity for definitions collected into DefMap and later IR layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ItemSource {
    pub file_id: FileId,
    pub kind: ItemSourceKind,
}

/// The storage layer that owns a source item payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum ItemSourceKind {
    ItemTree(ItemTreeRef),
    Generated(GeneratedItemRef),
    Body(BodyItemSourceRef),
}

impl ItemSource {
    pub fn item_tree(source: ItemTreeRef) -> Self {
        Self {
            file_id: source.file_id,
            kind: ItemSourceKind::ItemTree(source),
        }
    }

    pub fn generated(file_id: FileId, source: GeneratedItemRef) -> Self {
        Self {
            file_id,
            kind: ItemSourceKind::Generated(source),
        }
    }

    pub fn body(file_id: FileId, source: BodyItemSourceRef) -> Self {
        Self {
            file_id,
            kind: ItemSourceKind::Body(source),
        }
    }

    /// Temporary migration boundary until Semantic IR lowers through an item-source reader.
    pub fn as_item_tree(self) -> Option<ItemTreeRef> {
        match self.kind {
            ItemSourceKind::ItemTree(source) => Some(source),
            ItemSourceKind::Generated(_) => None,
            ItemSourceKind::Body(_) => None,
        }
    }

    /// Returns a source identity for an associated item in the same underlying item arena.
    // TODO: Do we need a generic item? This seem to exist for a very specific reason
    pub fn with_item(self, item: ItemTreeId) -> Self {
        let kind = match self.kind {
            ItemSourceKind::ItemTree(source) => ItemSourceKind::ItemTree(ItemTreeRef {
                file_id: source.file_id,
                item,
            }),
            ItemSourceKind::Generated(source) => ItemSourceKind::Generated(GeneratedItemRef {
                source: source.source,
                item,
            }),
            ItemSourceKind::Body(source) => ItemSourceKind::Body(BodyItemSourceRef {
                body: source.body,
                item,
            }),
        };

        Self {
            file_id: self.file_id,
            kind,
        }
    }
}

impl From<ItemTreeRef> for ItemSource {
    fn from(source: ItemTreeRef) -> Self {
        Self::item_tree(source)
    }
}

/// Item-tree-shaped payload retained for one declarative macro expansion.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct GeneratedSourceData {
    pub origin_file_id: FileId,
    pub origin_span: Span,
    pub origin_source: ItemTreeRef,
    pub top_level: Vec<ItemTreeId>,
    pub items: Arena<ItemTreeId, ItemNode>,
}

impl GeneratedSourceData {
    pub fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }

    pub fn shrink_to_fit(&mut self) {
        self.top_level.shrink_to_fit();
        for item in self.items.iter_mut() {
            item.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}
