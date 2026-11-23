//! Freeze the settled worklist before inference or source-aware reading can observe it.

use rg_arena::Arena;
use rg_std::Shrink as _;

use super::BodyStoreBuilder;
use crate::{PreparedBody, PreparedBodyStore};

impl BodyStoreBuilder<'_> {
    pub(super) fn finish(self) -> ajika::Result<PreparedBody> {
        assert_eq!(
            self.body_local_items.len(),
            self.lowered_bodies.bodies().len()
        );
        let (bodies, mut expansions) = self.lowered_bodies.into_parts();
        let (bodies, sources): (Vec<_>, Vec<_>) = bodies
            .into_vec()
            .into_iter()
            .map(|body| body.into_parts())
            .unzip();
        let mut data = PreparedBodyStore {
            bodies: Arena::from_vec(bodies),
            key: self.key,
            local_items: self.body_local_items,
        };
        let mut sources = Arena::from_vec(sources);
        data.shrink_to_fit();
        sources.shrink_to_fit();
        expansions.shrink_to_fit();
        PreparedBody::new(self.db, data, sources, expansions)
    }
}
