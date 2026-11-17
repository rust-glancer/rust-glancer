//! Item-tree-shaped source payloads stored inside lowered bodies.
//!
//! Lowering records these payloads mechanically; the build pipeline later turns them into
//! body-local DefMap and ItemStore facts.
//!
//! Occurrence IDs refer to the body's separate `BodySourceStore`. Moving these declarations
//! changes that table without changing their payloads or the collected local meanings.

use rg_arena::Arena;
use rg_ir_model::SourceId;
use rg_item_tree::{ItemNode, ItemTreeId};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Body-local item payload and the occurrence that locates its declaration.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItem {
    item: ItemNode,
    source: SourceId,
}

impl BodySourceItem {
    fn new(item: ItemNode, source: SourceId) -> Self {
        Self { item, source }
    }

    pub fn item(&self) -> &ItemNode {
        &self.item
    }

    pub fn source(&self) -> SourceId {
        self.source
    }
}

/// Item-tree-shaped declaration payloads owned by one lowered body.
///
/// Item IDs and occurrence IDs are unrelated. For example, `let value: User` records the `User`
/// occurrence without allocating an item node, while `struct Local;` records both an item and
/// its declaration occurrences.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItems {
    items: Arena<ItemTreeId, BodySourceItem>,
}

impl BodySourceItems {
    pub fn item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item).map(BodySourceItem::item)
    }

    pub fn source(&self, item: ItemTreeId) -> Option<SourceId> {
        self.items.get(item).map(BodySourceItem::source)
    }

    pub fn items(&self) -> &[BodySourceItem] {
        self.items.as_slice()
    }

    pub fn alloc(&mut self, item: ItemNode, source: SourceId) -> ItemTreeId {
        self.items.alloc(BodySourceItem::new(item, source))
    }
}
