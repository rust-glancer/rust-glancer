//! Assemble finalized bodies with their semantic sidecars and local declaration stores.

use rg_arena::Arena;

use crate::{CrateBodies, CurrentBody};

use super::CrateBodyBuildState;

impl CrateBodyBuildState<'_> {
    /// Finish request-local bodies without requiring their IDs to match worklist slots.
    pub(crate) fn finish_current(mut self) -> anyhow::Result<Vec<CurrentBody>> {
        rg_std::check_cancel!(self.cancellation, "finalize current bodies");
        let body_count = self.crate_bodies.bodies().len();
        anyhow::ensure!(
            body_count == self.body_refs.len()
                && body_count == self.body_facts.len()
                && body_count == self.body_local_items.len(),
            "current Body IR worklist stages produced misaligned body data",
        );

        let body_refs = self.body_refs.into_vec();
        let bodies = self.crate_bodies.into_bodies().into_vec();
        let facts = self.body_facts.into_vec();
        let local_items = self
            .body_local_items
            .iter_mut()
            .map(|items| {
                items
                    .take()
                    .expect("every current body should have collected body-local items")
            })
            .collect::<Vec<_>>();

        Ok(body_refs
            .into_iter()
            .zip(bodies)
            .zip(facts)
            .zip(local_items)
            .map(|(((body_ref, body), facts), local_items)| {
                CurrentBody::new(body_ref, body.into_body(), facts, local_items)
            })
            .collect())
    }

    pub(crate) fn finish(mut self) -> CrateBodies {
        debug_assert!(
            self.body_refs
                .iter_with_ids()
                .all(|(slot, body_ref)| body_ref.body == slot)
        );
        let mut body_local_items = Arena::with_capacity(self.body_local_items.len());
        for (body, items) in self.body_local_items.iter_mut_with_ids() {
            let items = items
                .take()
                .expect("every built body should have collected body-local items");
            let allocated = body_local_items.alloc(items);
            debug_assert_eq!(allocated, body);
        }
        let coverage = self.crate_bodies.coverage();
        let bodies = Arena::from_vec(
            self.crate_bodies
                .into_bodies()
                .into_vec()
                .into_iter()
                .map(|body| body.into_body())
                .collect(),
        );

        CrateBodies::from_build(coverage, bodies, self.body_facts, body_local_items)
    }
}
