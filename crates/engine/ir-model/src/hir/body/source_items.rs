//! Item-tree-shaped source payloads stored inside lowered bodies.
//!
//! Lowering records these payloads mechanically; the build pipeline later turns them into
//! body-local DefMap and ItemStore facts.

use rg_arena::Arena;
use wincode::{SchemaRead, SchemaWrite};

use crate::items::{ItemNode, ItemTreeId};
use rg_std::{MemorySize, Shrink};

/// Item-tree-shaped source payloads declared inside one lowered body.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceItems {
    items: Arena<ItemTreeId, ItemNode>,
}

impl BodySourceItems {
    pub fn item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item)
    }

    pub fn items(&self) -> &[ItemNode] {
        self.items.as_slice()
    }

    pub fn alloc(&mut self, item: ItemNode) -> ItemTreeId {
        self.items.alloc(item)
    }
}
