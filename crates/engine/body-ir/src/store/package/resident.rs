//! Dense Body IR representation used by resident packages and query code.

use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::{BodyId, CrateId};
use rg_semantic_ir::{ItemLookupIndex, ItemStore};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{BodyLocalItems, CrateBodiesCoverage, CrateBodiesStatus};
use crate::ir::body::ResolvedBodyData;

/// Lowered bodies for all semantic crates inside one package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageBodies {
    pub(crate) crates: Arena<CrateId, CrateBodies>,
}

impl PackageBodies {
    pub fn new(crates: Vec<CrateBodies>) -> Self {
        Self {
            crates: Arena::from_vec(crates),
        }
    }

    pub fn crates(&self) -> &[CrateBodies] {
        self.crates.as_slice()
    }

    pub fn crate_bodies(&self, crate_id: CrateId) -> Option<&CrateBodies> {
        self.crates.get(crate_id)
    }

    pub(crate) fn crates_mut(&mut self) -> &mut [CrateBodies] {
        self.crates.as_mut_slice()
    }
}

/// Resolved bodies for one semantic crate.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateBodies {
    pub(crate) coverage: CrateBodiesCoverage,
    pub(crate) semantic_index: ItemLookupIndex,
    pub(crate) bodies: Arena<BodyId, ResolvedBodyData>,
    pub(crate) body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl CrateBodies {
    pub(crate) fn with_coverage(coverage: CrateBodiesCoverage) -> Self {
        Self {
            coverage,
            semantic_index: ItemLookupIndex::default(),
            bodies: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub(crate) fn missing() -> Self {
        Self::with_coverage(CrateBodiesCoverage::Missing)
    }

    pub(crate) fn skipped_by_policy() -> Self {
        Self::with_coverage(CrateBodiesCoverage::SkippedByPolicy)
    }

    pub fn coverage(&self) -> CrateBodiesCoverage {
        self.coverage
    }

    pub fn status(&self) -> CrateBodiesStatus {
        self.coverage.status()
    }

    pub fn body(&self, body: BodyId) -> Option<&ResolvedBodyData> {
        self.bodies.get(body)
    }

    pub fn semantic_index(&self) -> &ItemLookupIndex {
        &self.semantic_index
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        self.body_local_items.get(body)
    }

    pub fn body_def_map(&self, body: BodyId) -> Option<&DefMap> {
        self.body_local_items(body).map(BodyLocalItems::def_map)
    }

    pub fn body_item_store(&self, body: BodyId) -> Option<&ItemStore> {
        self.body_local_items(body).map(BodyLocalItems::item_store)
    }

    pub fn bodies(&self) -> &[ResolvedBodyData] {
        self.bodies.as_slice()
    }

    pub(crate) fn alloc_body(&mut self, data: ResolvedBodyData) -> BodyId {
        self.bodies.alloc(data)
    }

    pub(crate) fn set_body_local_items(&mut self, items: Vec<BodyLocalItems>) {
        debug_assert_eq!(
            self.bodies.len(),
            items.len(),
            "every built body should have finalized body-local items"
        );
        self.body_local_items = Arena::from_vec(items);
    }

    pub(crate) fn set_semantic_index(&mut self, index: ItemLookupIndex) {
        self.semantic_index = index;
    }

    pub(crate) fn bodies_mut(&mut self) -> &mut [ResolvedBodyData] {
        self.bodies.as_mut_slice()
    }
}
