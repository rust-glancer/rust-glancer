//! Item-tree-shaped source payloads stored inside lowered bodies.
//!
//! Lowering records these payloads mechanically; the build pipeline later turns them into
//! body-local DefMap and ItemStore facts.
//!
//! The same store owns occurrence locations for declaration and type syntax inside the body,
//! including type annotations that do not introduce a local item.

use rg_arena::Arena;
use rg_ir_model::{SourceId, SourceLocation};
use rg_item_tree::{DeclarationSourceStore, ItemNode, ItemTreeId, SourceLocations};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::BodySource;

/// Body-local item payload with the source provenance of the syntax that produced it.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItem {
    item: ItemNode,
    source: BodySource,
}

impl BodySourceItem {
    fn new(item: ItemNode, source: BodySource) -> Self {
        Self { item, source }
    }

    pub fn item(&self) -> &ItemNode {
        &self.item
    }

    pub fn source(&self) -> BodySource {
        self.source
    }
}

/// Item-tree-shaped source payloads and occurrence locations owned by one lowered body.
///
/// `items` is addressed by [`ItemTreeId`]; `locations` is addressed by [`SourceId`]. Their indices
/// are unrelated. For example, `let value: User` records the `User` occurrence without allocating
/// an item node, while `struct Local;` records both an item and its declaration occurrences.
///
/// Body lowering resolves locations while it still has the syntax and macro input maps. The
/// stored [`SourceLocation`] values therefore already belong to this body's source snapshot,
/// whether it was built from saved text or from a current editor document.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItems {
    items: Arena<ItemTreeId, BodySourceItem>,
    pub(crate) locations: DeclarationSourceStore<SourceLocation>,
}

impl BodySourceItems {
    pub fn item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item).map(BodySourceItem::item)
    }

    pub fn source(&self, item: ItemTreeId) -> Option<BodySource> {
        self.items.get(item).map(BodySourceItem::source)
    }

    pub fn items(&self) -> &[BodySourceItem] {
        self.items.as_slice()
    }

    pub fn alloc(&mut self, item: ItemNode, source: BodySource) -> ItemTreeId {
        self.items.alloc(BodySourceItem::new(item, source))
    }
}

impl SourceLocations for BodySourceItems {
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation> {
        self.locations.get(occurrence).copied()
    }
}
