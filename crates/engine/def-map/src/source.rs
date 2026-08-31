use std::collections::HashMap;

use rg_arena::Arena;
use rg_item_tree::{ItemNode, ItemTreeId, ItemTreeRef};
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::BodyRef;

/// Stable identifier of one macro expansion payload produced during crate construction.
///
/// The id remains part of compact source provenance after the payload is discarded, just as an
/// [`ItemTreeRef`] remains useful after the transient ItemTree phase has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct GeneratedSourceId(pub usize);

impl rg_arena::ArenaId for GeneratedSourceId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Crate-local source identity for one generated item.
///
/// During Semantic IR lowering this addresses [`GeneratedItemStore`]. Later phases retain it only
/// as provenance and use the semantic declaration data copied during lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct GeneratedItemRef {
    pub source: GeneratedSourceId,
    pub item: ItemTreeId,
}

/// Body-local reference to one item-tree-shaped source payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyItemSourceRef {
    pub body: BodyRef,
    pub item: ItemTreeId,
}

/// Durable source identity for definitions collected into DefMap and later IR layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ItemSource {
    pub file_id: FileId,
    pub kind: ItemSourceKind,
}

/// The storage layer that owns a source item payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
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

    /// Returns the ordinary item-tree source when this definition did not come from expansion or
    /// a body-local item arena.
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

/// Item-tree-shaped payload produced for one declarative macro expansion.
///
/// This data is intentionally construction-only. DefMap uses it while collecting scopes and
/// Semantic IR copies the declaration facts it needs before the surrounding store is dropped.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub(crate) struct GeneratedSourceData {
    pub(crate) origin_file_id: FileId,
    pub(crate) origin_span: Span,
    pub(crate) origin_source: ItemTreeRef,
    pub(crate) top_level: Vec<ItemTreeId>,
    pub(crate) items: Arena<ItemTreeId, ItemNode>,
}

impl GeneratedSourceData {
    pub(crate) fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }
}

/// Crate-local generated declarations retained only between DefMap and Semantic IR construction.
///
/// Macro expansion needs item-tree-shaped declarations while it discovers modules, definitions,
/// imports, and associated items. Those declarations are substantially larger than the resulting
/// semantic facts, so this store travels beside the frozen DefMap instead of becoming part of it.
#[derive(Debug, Clone, Default, MemorySize, Shrink)]
pub struct GeneratedItemStore {
    sources: Arena<GeneratedSourceId, GeneratedSourceData>,
    /// Generated associated items keyed by the macro call they replace.
    ///
    /// For `impl User { methods!(); }`, the call source maps to its generated functions, types,
    /// consts, and any retained nested macro calls.
    associated_macro_expansions: HashMap<ItemSource, Vec<ItemSource>>,
}

impl GeneratedItemStore {
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.associated_macro_expansions.is_empty()
    }

    pub(crate) fn alloc_source(&mut self, source: GeneratedSourceData) -> GeneratedSourceId {
        self.sources.alloc(source)
    }

    pub(crate) fn source(&self, source: GeneratedSourceId) -> Option<&GeneratedSourceData> {
        self.sources.get(source)
    }

    pub(crate) fn insert_associated_macro_expansion(
        &mut self,
        call: ItemSource,
        generated_items: Vec<ItemSource>,
    ) {
        self.associated_macro_expansions
            .insert(call, generated_items);
    }

    pub fn item(&self, item: GeneratedItemRef) -> Option<&ItemNode> {
        self.sources
            .get(item.source)
            .and_then(|source| source.item(item.item))
    }

    pub fn associated_macro_expansion(&self, call: ItemSource) -> Option<&[ItemSource]> {
        self.associated_macro_expansions
            .get(&call)
            .map(Vec::as_slice)
    }
}
