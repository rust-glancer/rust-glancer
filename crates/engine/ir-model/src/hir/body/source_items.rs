//! Item-tree-shaped source payloads stored inside lowered bodies.
//!
//! Lowering records these payloads mechanically; the build pipeline later turns them into
//! body-local DefMap and ItemStore facts.

use rg_arena::Arena;
use wincode::{SchemaRead, SchemaWrite};

use crate::items::{ItemNode, ItemTreeId};
use rg_std::{MemorySize, Shrink};

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

/// Item-tree-shaped source payloads declared inside one lowered body.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItems {
    items: Arena<ItemTreeId, BodySourceItem>,
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
