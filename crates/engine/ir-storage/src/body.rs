use rg_ir_model::{ImplId, TraitRef, TypeDefRef};

use crate::{DefMap, ItemStore};
use rg_std::{MemorySize, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

/// Finalized body-local DefMap and semantic-shaped item facts for one body.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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

    pub fn set_impl_header_facts(
        &mut self,
        id: ImplId,
        resolved_self_tys: UniqueVec<TypeDefRef>,
        resolved_trait_refs: UniqueVec<TraitRef>,
    ) -> Option<()> {
        self.item_store
            .set_impl_header_facts(id, resolved_self_tys, resolved_trait_refs)
    }

    pub fn shrink_to_fit(&mut self) {
        self.def_map.shrink_to_fit();
        self.item_store.shrink_to_fit();
    }
}
