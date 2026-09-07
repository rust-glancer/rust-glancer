//! Body analysis held in memory, grouped by package and Cargo target.

use std::sync::Arc;

use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::{BodyId, CrateId};
use rg_semantic_ir::ItemStore;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{BodyLocalItems, CrateBodiesCoverage, CrateBodiesStatus, PackageBodiesCoverage};
use crate::{BodyData, BodyFacts, BodyView};

/// In-memory body analysis for the Cargo targets in one package, indexed by [`CrateId`].
///
/// For example, a library and an integration test have separate [`CrateBodies`] entries even
/// when they share source files. Cloning this collection shares each entry through an `Arc`.
/// Replacing one target then leaves the other targets and readers of older entries using their
/// existing body data.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageBodies {
    crates: Arena<CrateId, Arc<CrateBodies>>,
}

impl Shrink for PackageBodies {
    fn shrink_to_fit(&mut self) {
        self.crates.shrink_to_fit();
        for bodies in self.crates.iter_mut() {
            // Only shrink body data we own exclusively. Copying shared entries just to shrink
            // their allocations would duplicate other targets' bodies and existing readers' data.
            if let Some(bodies) = Arc::get_mut(bodies) {
                Shrink::shrink_to_fit(bodies);
            }
        }
    }
}

impl PackageBodies {
    pub fn new(crates: Vec<CrateBodies>) -> Self {
        Self {
            crates: Arena::from_vec(crates.into_iter().map(Arc::new).collect()),
        }
    }

    pub fn crates(&self) -> &[Arc<CrateBodies>] {
        self.crates.as_slice()
    }

    pub fn crate_bodies(&self, crate_id: CrateId) -> Option<&CrateBodies> {
        self.crates.get(crate_id).map(AsRef::as_ref)
    }

    pub fn replace_crate(&mut self, crate_id: CrateId, bodies: CrateBodies) -> Option<()> {
        *self.crates.get_mut(crate_id)? = Arc::new(bodies);
        Some(())
    }

    pub(crate) fn coverage(&self) -> PackageBodiesCoverage {
        PackageBodiesCoverage::from_crates(
            self.crates()
                .iter()
                .map(|bodies| bodies.coverage().clone())
                .collect(),
        )
    }
}

/// Stores the analyzed bodies for one crate and records which files were analyzed.
///
/// Each [`BodyId`] selects matching [`BodyData`], [`BodyFacts`] and [`BodyLocalItems`]: the body's
/// expressions and bindings, inferred information, and declarations written inside it. Rebuilding
/// with more source files can change those ids, so these three stores must be replaced together.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateBodies {
    coverage: CrateBodiesCoverage,
    bodies: Arena<BodyId, BodyData>,
    facts: Arena<BodyId, BodyFacts>,
    body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl CrateBodies {
    pub(crate) fn empty(coverage: CrateBodiesCoverage) -> Self {
        Self::from_resident_parts(coverage, Arena::new(), Arena::new(), Arena::new())
    }

    pub(crate) fn from_build(
        coverage: CrateBodiesCoverage,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    ) -> Self {
        debug_assert!(coverage.is_materialized());
        Self::from_resident_parts(coverage, bodies, facts, body_local_items)
    }

    /// Combine body data, facts and local declarations that use the same [`BodyId`] indices.
    ///
    /// [`PackageBodies`] also keeps entries for targets whose analysis is missing or skipped by
    /// policy. Empty arenas with those coverage values are valid here too.
    pub(crate) fn from_resident_parts(
        coverage: CrateBodiesCoverage,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    ) -> Self {
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
            bodies,
            facts,
            body_local_items,
        }
    }

    pub fn coverage(&self) -> &CrateBodiesCoverage {
        &self.coverage
    }

    pub fn status(&self) -> CrateBodiesStatus {
        self.coverage().status()
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        Some(BodyView::new(self.bodies.get(body)?, self.facts.get(body)?))
    }

    pub(crate) fn body_facts(&self, body: BodyId) -> Option<&BodyFacts> {
        self.facts.get(body)
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
            .map(move |(body, data)| (body, BodyView::new(data, &self.facts[body])))
    }
}
