//! Item-tree-shaped source payloads stored inside lowered bodies.
//!
//! Lowering records these payloads mechanically; the build pipeline later turns them into
//! body-local DefMap and ItemStore facts.

use rg_arena::Arena;
use rg_memsize::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::items::{ItemNode, ItemTreeId};

/// Item-tree-shaped source payloads declared inside one lowered body.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
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

    pub fn shrink_to_fit(&mut self) {
        for item in self.items.iter_mut() {
            item.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}
