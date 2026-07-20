use crate::ItemStore;
use rg_arena::Arena;
use rg_ir_model::CrateId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Semantic IR for one Cargo package.
///
/// Crate IDs are assigned by DefMap and reused by later semantic phases in the package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageIr {
    pub(crate) crates: Arena<CrateId, ItemStore>,
}

impl PackageIr {
    pub(crate) fn new(crates: Vec<ItemStore>) -> Self {
        Self {
            crates: Arena::from_vec(crates),
        }
    }

    /// Returns all crate item stores for this package in crate-id order.
    pub fn crates(&self) -> &[ItemStore] {
        self.crates.as_slice()
    }

    pub fn crate_items(&self, crate_id: CrateId) -> Option<&ItemStore> {
        self.crates.get(crate_id)
    }

    pub(crate) fn crate_items_mut(&mut self, crate_id: CrateId) -> Option<&mut ItemStore> {
        self.crates.get_mut(crate_id)
    }
}
