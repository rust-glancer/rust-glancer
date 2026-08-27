use std::collections::HashMap;

use crate::{ItemLookupIndex, ItemStore, TraitImplSelfHead};
use rg_arena::Arena;
use rg_ir_model::{CrateId, ImplRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Semantic IR for one Cargo package.
///
/// Crate IDs are assigned by DefMap and reused by later semantic phases in the package. Every item
/// store has an aligned declaration-local lookup index under the same crate id. The index is built
/// after impl headers are resolved, because their receiver and trait identities provide its keys.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageIr {
    pub(crate) crates: Arena<CrateId, ItemStore>,
    pub(crate) lookup_indexes: Arena<CrateId, ItemLookupIndex>,
}

impl PackageIr {
    pub(crate) fn new(crates: Vec<ItemStore>) -> Self {
        let lookup_indexes = (0..crates.len())
            .map(|_| ItemLookupIndex::default())
            .collect();
        Self {
            crates: Arena::from_vec(crates),
            lookup_indexes: Arena::from_vec(lookup_indexes),
        }
    }

    /// Returns all crate item stores for this package in crate-id order.
    pub fn crates(&self) -> &[ItemStore] {
        self.crates.as_slice()
    }

    pub fn crate_items(&self, crate_id: CrateId) -> Option<&ItemStore> {
        self.crates.get(crate_id)
    }

    /// Returns the declaration-local lookup index aligned with this crate's item store.
    pub fn crate_lookup_index(&self, crate_id: CrateId) -> Option<&ItemLookupIndex> {
        self.lookup_indexes.get(crate_id)
    }

    pub(crate) fn crate_items_mut(&mut self, crate_id: CrateId) -> Option<&mut ItemStore> {
        self.crates.get_mut(crate_id)
    }

    /// Rebuild aligned crate-local indexes after impl-header resolution changes their lookup keys.
    pub(crate) fn rebuild_lookup_indexes(
        &mut self,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
    ) {
        debug_assert_eq!(self.crates.len(), self.lookup_indexes.len());
        for (crate_id, store) in self.crates.iter_with_ids() {
            self.lookup_indexes[crate_id] = ItemLookupIndex::build_from_store(store, self_heads);
        }
    }
}
