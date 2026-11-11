use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{CrateId, CrateRef, ImplRef};

use crate::{ItemLookupFacts, ItemLookupIndex, ItemStore, TraitImplSelfHead};

/// Completed declaration and lookup arenas for one target in kernel fixtures.
#[derive(Debug)]
pub struct SemanticCrate {
    items: ItemStore,
    lookup_index: ItemLookupIndex,
}

impl SemanticCrate {
    fn new(items: ItemStore) -> Self {
        Self {
            items,
            lookup_index: ItemLookupIndex::default(),
        }
    }

    pub fn items(&self) -> &ItemStore {
        &self.items
    }

    pub fn lookup_index(&self) -> &ItemLookupIndex {
        &self.lookup_index
    }
}

/// Local semantic arenas grouped by Cargo target slot.
#[derive(Debug)]
pub struct SemanticPackage {
    pub(crate) crates: Arena<CrateId, SemanticCrate>,
}

impl SemanticPackage {
    pub(crate) fn new(crates: Vec<ItemStore>) -> Self {
        Self {
            crates: Arena::from_vec(crates.into_iter().map(SemanticCrate::new).collect()),
        }
    }

    /// Returns all semantic crates in crate-id order.
    pub fn crates(&self) -> &[SemanticCrate] {
        self.crates.as_slice()
    }

    fn semantic_crate(&self, crate_id: CrateId) -> Option<&SemanticCrate> {
        self.crates.get(crate_id)
    }

    pub fn crate_items(&self, crate_id: CrateId) -> Option<&ItemStore> {
        self.semantic_crate(crate_id).map(SemanticCrate::items)
    }

    /// Returns the declaration-local lookup index aligned with this crate's item store.
    pub fn crate_lookup_index(&self, crate_id: CrateId) -> Option<&ItemLookupIndex> {
        self.semantic_crate(crate_id)
            .map(SemanticCrate::lookup_index)
    }

    pub(crate) fn crate_items_mut(&mut self, crate_id: CrateId) -> Option<&mut ItemStore> {
        self.crates
            .get_mut(crate_id)
            .map(|semantic_crate| &mut semantic_crate.items)
    }

    /// Rebuild crate-local candidate tables after impl-header resolution changes their lookup keys.
    /// Attach lookup facts computed from the same saved declarations as these tables.
    pub(crate) fn rebuild_lookup_indexes(
        &mut self,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
        lookup_facts: &HashMap<CrateRef, ItemLookupFacts>,
    ) {
        for semantic_crate in self.crates.iter_mut() {
            let facts = lookup_facts
                .get(&semantic_crate.items.crate_ref())
                .expect("rebuilt crate has indexed lookup facts")
                .clone();
            semantic_crate.lookup_index =
                ItemLookupIndex::build_from_store(&semantic_crate.items, self_heads, facts);
        }
    }
}
