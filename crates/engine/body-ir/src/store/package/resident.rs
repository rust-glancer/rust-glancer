//! Dense Body IR representation used by resident packages and query code.

use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::{BodyData, BodyId, CrateId};
use rg_semantic_ir::{ItemLookupIndex, ItemStore};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{BodyLocalItems, CrateBodiesCoverage, CrateBodiesStatus};
use crate::{BodyFacts, BodyView};

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
}

/// Immutable body shapes and their aligned semantic sidecars for one semantic crate.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateBodies {
    pub(crate) coverage: CrateBodiesCoverage,
    pub(crate) semantic_index: ItemLookupIndex,
    pub(crate) bodies: Arena<BodyId, BodyData>,
    pub(crate) facts: Arena<BodyId, BodyFacts>,
    pub(crate) body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl CrateBodies {
    pub(crate) fn empty(coverage: CrateBodiesCoverage) -> Self {
        debug_assert!(
            !coverage.is_materialized(),
            "materialized crate bodies should be constructed from resolved build output",
        );
        Self {
            coverage,
            semantic_index: ItemLookupIndex::default(),
            bodies: Arena::new(),
            facts: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub(crate) fn from_build(
        coverage: CrateBodiesCoverage,
        semantic_index: ItemLookupIndex,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    ) -> Self {
        debug_assert!(coverage.is_materialized());
        debug_assert_eq!(bodies.len(), facts.len());
        debug_assert_eq!(bodies.len(), body_local_items.len());
        debug_assert!(
            bodies
                .iter()
                .zip(&facts)
                .all(|(body, facts)| facts.is_aligned_with(body)),
        );
        Self {
            coverage,
            semantic_index,
            bodies,
            facts,
            body_local_items,
        }
    }

    pub fn coverage(&self) -> CrateBodiesCoverage {
        self.coverage
    }

    pub fn status(&self) -> CrateBodiesStatus {
        self.coverage.status()
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        Some(BodyView::new(self.bodies.get(body)?, self.facts.get(body)?))
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

    pub fn bodies(&self) -> &[BodyData] {
        self.bodies.as_slice()
    }

    /// Iterate bodies in stable `BodyId` order with their aligned semantic facts.
    pub fn body_views(&self) -> impl Iterator<Item = (BodyId, BodyView<'_>)> {
        self.bodies
            .iter_with_ids()
            .map(|(body, data)| (body, BodyView::new(data, &self.facts[body])))
    }
}
