use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::BodyId;
use rg_semantic_ir::ItemStore;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{BodyData, BodyFacts, BodyLocalItems, BodyView};

/// Aligned syntax, semantic facts and declarations for one completed body worklist.
///
/// Each [`BodyId`] selects matching [`BodyData`], [`BodyFacts`] and [`BodyLocalItems`]: the body's
/// expressions and bindings, inferred information, and declarations written inside it. Rebuilding
/// with more source files can change those ids, so these three stores must be replaced together.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyStore {
    bodies: Arena<BodyId, BodyData>,
    facts: Arena<BodyId, BodyFacts>,
    body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl BodyStore {
    pub(crate) fn new(
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
            bodies,
            facts,
            body_local_items,
        }
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        Some(BodyView::new(self.bodies.get(body)?, self.facts.get(body)?))
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
