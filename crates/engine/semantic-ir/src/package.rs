use std::collections::HashMap;

use crate::{ItemLookupIndex, ItemStore, TraitImplSelfHead};
use rg_arena::Arena;
use rg_ir_model::{CrateId, ImplRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Semantic declarations and their aligned lookup index for one crate.
///
/// The resident model keeps the pair explicit under one crate id. Persisted storage frames the two
/// halves independently: ordinary declaration queries need the item store, while visibility-wide
/// candidate lookup needs only the smaller index.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
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

    /// Rejoin independently persisted declaration and lookup payloads.
    pub fn from_storage_parts(items: ItemStore, lookup_index: ItemLookupIndex) -> Self {
        Self {
            items,
            lookup_index,
        }
    }
}

/// Package directory needed before any semantic crate is decoded.
///
/// Semantic crates use the dense [`CrateId`] slots assigned by DefMap. The count is enough to
/// allocate request-local item and lookup-index cells; declarations themselves remain in the crate
/// payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub struct SemanticPackageManifest {
    crate_count: usize,
}

impl SemanticPackageManifest {
    pub fn crate_count(self) -> usize {
        self.crate_count
    }
}

/// Semantic declarations and lookup indexes for one Cargo package.
///
/// Crate IDs are assigned by DefMap and reused by later semantic phases in the package. Each arena
/// slot owns the declaration store and the lookup index built from its resolved impl headers.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct SemanticPackage {
    pub(crate) crates: Arena<CrateId, SemanticCrate>,
}

impl SemanticPackage {
    pub(crate) fn new(crates: Vec<ItemStore>) -> Self {
        Self {
            crates: Arena::from_vec(crates.into_iter().map(SemanticCrate::new).collect()),
        }
    }

    /// Rebuilds the broad resident package from all crate-granular storage units.
    ///
    /// The crate payloads must fill every dense slot declared by the manifest. Exact query paths do
    /// not need this operation: they can read one crate's items or lookup index independently.
    pub fn from_storage_parts(
        manifest: SemanticPackageManifest,
        crates: Vec<SemanticCrate>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            crates.len() == manifest.crate_count,
            "Semantic IR storage has {} crates, manifest declares {}",
            crates.len(),
            manifest.crate_count,
        );
        Ok(Self {
            crates: Arena::from_vec(crates),
        })
    }

    /// Builds the compact directory stored before the crate-granular Semantic IR payloads.
    pub fn manifest(&self) -> SemanticPackageManifest {
        SemanticPackageManifest {
            crate_count: self.crates.len(),
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

    /// Rebuild aligned crate-local indexes after impl-header resolution changes their lookup keys.
    pub(crate) fn rebuild_lookup_indexes(
        &mut self,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
    ) {
        for semantic_crate in self.crates.iter_mut() {
            semantic_crate.lookup_index =
                ItemLookupIndex::build_from_store(&semantic_crate.items, self_heads);
        }
    }
}
