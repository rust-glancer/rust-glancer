//! Outputs of one selected-package DefMap construction session.
//!
//! Frozen DefMap data and generated declaration payloads deliberately have different lifetimes.
//! The former becomes saved project state. The latter exists only long enough for Semantic IR to
//! copy signatures and declaration facts, then it is dropped before Body IR or cache writing.

use std::collections::HashMap;

use rg_ir_model::CrateRef;
use rg_std::MemorySize;

use crate::{DefMapDb, GeneratedItemStore};

/// Generated declaration stores for the crates rebuilt by one DefMap session.
#[derive(Debug, Clone, Default, MemorySize)]
pub struct GeneratedItemStores {
    crates: HashMap<CrateRef, GeneratedItemStore>,
}

impl GeneratedItemStores {
    pub fn crate_items(&self, crate_ref: CrateRef) -> Option<&GeneratedItemStore> {
        self.crates.get(&crate_ref)
    }

    pub(crate) fn insert(&mut self, crate_ref: CrateRef, items: GeneratedItemStore) {
        let previous = self.crates.insert(crate_ref, items);
        debug_assert!(previous.is_none(), "generated crate items inserted twice");
    }
}

/// Frozen DefMap replacement plus the transient declarations needed by the next build phase.
#[derive(Debug, MemorySize)]
pub struct DefMapBuildOutput {
    def_map: DefMapDb,
    generated_items: GeneratedItemStores,
}

impl DefMapBuildOutput {
    pub(crate) fn new(def_map: DefMapDb, generated_items: GeneratedItemStores) -> Self {
        Self {
            def_map,
            generated_items,
        }
    }

    pub fn into_parts(self) -> (DefMapDb, GeneratedItemStores) {
        (self.def_map, self.generated_items)
    }
}
