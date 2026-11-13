//! Freeze the settled worklist before inference or source-aware reading can observe it.

use rg_arena::Arena;
use rg_ir_model::BodyId;

use super::BodyStoreBuilder;
use crate::{BodySourceStore, PreparedBodyStore};

impl BodyStoreBuilder<'_> {
    pub(crate) fn finish(self) -> (PreparedBodyStore, Arena<BodyId, BodySourceStore>) {
        assert_eq!(self.body_refs.len(), self.lowered_bodies.bodies().len());
        assert_eq!(
            self.body_local_items.len(),
            self.lowered_bodies.bodies().len()
        );
        let (bodies, sources): (Vec<_>, Vec<_>) = self
            .lowered_bodies
            .into_bodies()
            .into_vec()
            .into_iter()
            .map(|body| body.into_parts())
            .unzip();
        (
            PreparedBodyStore {
                bodies: Arena::from_vec(bodies),
                body_refs: self.body_refs,
                local_items: self.body_local_items,
                initializer: None,
            },
            Arena::from_vec(sources),
        )
    }
}
