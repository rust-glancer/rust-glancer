use crate::{DefMap, ItemStore};

/// Finalized body-local DefMap and semantic-shaped item facts for one body.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct BodyLocalItems {
    def_map: DefMap,
    item_store: ItemStore,
}

impl BodyLocalItems {
    pub fn new(def_map: DefMap, item_store: ItemStore) -> Self {
        Self {
            def_map,
            item_store,
        }
    }

    pub fn def_map(&self) -> &DefMap {
        &self.def_map
    }

    pub fn item_store(&self) -> &ItemStore {
        &self.item_store
    }

    pub fn item_store_mut(&mut self) -> &mut ItemStore {
        &mut self.item_store
    }

    pub fn shrink_to_fit(&mut self) {
        self.def_map.shrink_to_fit();
        self.item_store.shrink_to_fit();
    }
}
