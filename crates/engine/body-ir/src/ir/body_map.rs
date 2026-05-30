use rg_arena::Arena;
use rg_item_tree::{ItemNode, ItemTreeId};

/// Item-tree-shaped source payloads declared inside one function body.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodySourceItems {
    pub(crate) items: Arena<ItemTreeId, ItemNode>,
}

impl BodySourceItems {
    pub fn item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item)
    }

    pub fn items(&self) -> &[ItemNode] {
        self.items.as_slice()
    }

    pub(crate) fn alloc(&mut self, item: ItemNode) -> ItemTreeId {
        self.items.alloc(item)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        for item in self.items.iter_mut() {
            item.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}
