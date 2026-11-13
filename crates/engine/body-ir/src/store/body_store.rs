//! An operation-local join of independently stored preparation, inference, and locations.

use std::sync::Arc;

use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::BodyId;

use crate::{
    BodyData, BodyFactsStore, BodyLocalItems, BodySourceStore, BodyView, PreparedBodyStore,
};

/// Shared payloads for one outer owner. This join is never serialized: the three payloads have
/// independent equality and persistence boundaries, and are released with their query operation.
#[derive(Debug, Clone)]
pub struct BodyStore {
    pub(crate) prepared: Arc<PreparedBodyStore>,
    pub(crate) facts: Arc<BodyFactsStore>,
    pub(crate) sources: Arc<Arena<BodyId, BodySourceStore>>,
}

impl BodyStore {
    pub(crate) fn new(
        prepared: Arc<PreparedBodyStore>,
        facts: Arc<BodyFactsStore>,
        sources: Arc<Arena<BodyId, BodySourceStore>>,
    ) -> Self {
        assert_eq!(prepared.bodies.len(), facts.bodies.len());
        assert_eq!(prepared.bodies.len(), sources.len());
        Self {
            prepared,
            facts,
            sources,
        }
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        Some(BodyView::new(
            self.prepared.bodies.get(body)?,
            self.facts.bodies.get(body)?,
            self.sources.get(body)?,
        ))
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        self.prepared.local_items.get(body)
    }

    pub fn body_def_map(&self, body: BodyId) -> Option<&DefMap> {
        self.body_local_items(body).map(BodyLocalItems::def_map)
    }

    pub fn bodies(&self) -> &[BodyData] {
        self.prepared.bodies.as_slice()
    }

    pub fn body_views(&self) -> impl Iterator<Item = (BodyId, BodyView<'_>)> {
        self.prepared
            .bodies
            .iter_with_ids()
            .map(move |(body, data)| {
                (
                    body,
                    BodyView::new(data, &self.facts.bodies[body], &self.sources[body]),
                )
            })
    }
}
