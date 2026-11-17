//! Preparation of one outer owner and the nested bodies discovered in its local declarations.
//!
//! Local-item collection expands the worklist before semantic stages derive body facts. Each stage
//! updates this same worklist; finalization separates immutable meaning from source occurrences.

mod finish;
mod local_items;
mod semantics;

use std::time::{Duration, Instant};

use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::DefinitionReader;
use rg_ir_model::{BodyId, BodyKey, BodyRef};
use rg_semantic_ir::{ItemLookupQuery, ItemReader};
use rg_source::QueryContext;
use rg_std::CancellationToken;
use rg_text::RustEdition;

use super::lower::LoweredBodies;
use crate::{BodyLocalItems, BodySourceStore, PreparedBodyStore};

// Phase logs explain a slow owner preparation without emitting one event for every normal phase.
const SLOW_PREPARATION_PHASE: Duration = Duration::from_secs(1);

/// Time spent in each shared semantic stage.
#[derive(Default)]
pub(super) struct BodySemanticTimings {
    pub(super) impl_headers: Duration,
    pub(super) pattern_bindings: Duration,
}

/// Settles declarations, nested identities, and binding arenas before publishing an owner's data.
pub(super) struct BodyStoreBuilder<'source> {
    key: BodyKey,
    db: &'source QueryContext<'source>,
    edition: RustEdition,
    lowered_bodies: LoweredBodies,
    body_local_items: Arena<BodyId, BodyLocalItems>,
    cancellation: CancellationToken,
}

impl<'source> BodyStoreBuilder<'source> {
    pub(super) fn new(
        key: BodyKey,
        db: &'source QueryContext<'source>,
        edition: RustEdition,
        lowered_bodies: LoweredBodies,
        cancellation: CancellationToken,
    ) -> Self {
        assert_eq!(
            lowered_bodies.bodies().len(),
            1,
            "preparation starts with one outer owner"
        );
        Self {
            key,
            db,
            edition,
            lowered_bodies,
            body_local_items: Arena::new(),
            cancellation,
        }
    }

    /// Prepare an outer owner, assigning nested identities in its worklist order.
    ///
    /// The owner gets its own use-site visibility and solver answers. Candidate answers, lookup
    /// preparation, and canonical declarations are shared by the active query operation.
    pub(super) fn prepare(
        mut self,
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
        cfg: CfgEvaluator<'_>,
    ) -> anyhow::Result<(PreparedBodyStore, Arena<BodyId, BodySourceStore>)> {
        let span = tracing::debug_span!(
            "body_ir_preparation",
            rg.crate_id = self.key.krate.crate_id.0,
        );
        let _entered = span.enter();
        let preparation_started = Instant::now();

        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        let phase_started = Instant::now();
        self.materialize_body_local_items(def_map, semantic_ir, cfg)?;
        let elapsed = phase_started.elapsed();
        let body_local_items_ms = elapsed.as_millis();
        if elapsed >= SLOW_PREPARATION_PHASE {
            tracing::debug!(
                phase = "body_local_items",
                elapsed_ms = elapsed.as_millis(),
                body_count = self.lowered_bodies.bodies().len(),
                "slow Body IR preparation phase"
            );
        }

        // Build the visibility query before any body query starts. Declaration-local indexes live
        // in Semantic IR; `BodyBuildQuerySource` adds items declared inside this body to that lookup.
        let phase_started = Instant::now();
        let item_lookup_query =
            ItemLookupQuery::build_from(*semantic_ir, self.key.krate, &self.cancellation)?;
        let elapsed = phase_started.elapsed();
        let item_lookup_query_ms = elapsed.as_millis();
        if elapsed >= SLOW_PREPARATION_PHASE {
            tracing::debug!(
                phase = "item_lookup_query",
                elapsed_ms = elapsed.as_millis(),
                "slow Body IR preparation phase"
            );
        }
        let semantic_timings = self.prepare_semantics(def_map, semantic_ir, &item_lookup_query)?;
        let body_local_impl_headers_ms = semantic_timings.impl_headers.as_millis();
        let pattern_bindings_ms = semantic_timings.pattern_bindings.as_millis();

        // Freeze the settled body arenas and their local declarations before inference reads them.
        let body_count = self.lowered_bodies.bodies().len();
        let finish_started = Instant::now();
        rg_std::check_cancel!(self.cancellation, "finalize body preparation");
        let bodies = self.finish();
        let finish_ms = finish_started.elapsed().as_millis();
        tracing::trace!(
            body_count,
            body_local_items_ms,
            item_lookup_query_ms,
            body_local_impl_headers_ms,
            pattern_bindings_ms,
            finish_ms,
            total_ms = preparation_started.elapsed().as_millis(),
            "Body IR preparation finished"
        );
        Ok(bodies)
    }

    fn body_ref(&self, body: BodyId) -> BodyRef {
        self.key.body(body)
    }
}
