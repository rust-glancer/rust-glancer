//! Shared state and build sequence for saved crates and current body worklists.
//!
//! Local-item collection expands the worklist before semantic stages derive body facts. Each stage
//! updates this same owner; finalization then produces saved storage or request-local bodies.

mod finish;
mod local_items;
mod semantics;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use rg_arena::Arena;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{BodyId, BodyRef, CrateRef};
use rg_semantic_ir::{CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, SemanticIrReadTxn};
use rg_text::NameInterner;

use super::lower::{BodyTaskSource, LoweredCrateBodies};
use crate::{BodyFacts, BodyLocalItems, CrateBodies};

// Phase logs explain a slow crate-level build without emitting one event for every normal phase.
const SLOW_CRATE_RESOLUTION_PHASE: Duration = Duration::from_secs(1);
/// Semantic stages shared by saved crate builds and selected current-body builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodySemanticStage {
    ImplHeaders,
    PatternBindings,
    Bodies,
}

/// Time spent in each shared semantic stage.
pub(super) struct BodySemanticTimings {
    pub(super) impl_headers: Duration,
    pub(super) pattern_bindings: Duration,
    pub(super) bodies: Duration,
}

/// Coordinates all body-local facts needed to resolve one crate's bodies.
pub(super) struct CrateBodyBuildState<'crate_data> {
    crate_ref: CrateRef,
    parse_package: &'crate_data rg_parse::Package,
    crate_bodies: LoweredCrateBodies,
    /// Project identities for the bodies in the temporary worklist.
    ///
    /// Saved builds use the same ID for both arenas. A selected current build may reuse one saved
    /// identity or allocate a request-only identity, so its worklist slot cannot stand in for a
    /// `BodyRef`.
    body_refs: Arena<BodyId, BodyRef>,
    /// Worklist slot for each project or request-only body identity.
    body_slots: HashMap<BodyRef, BodyId>,
    body_facts: Arena<BodyId, BodyFacts>,
    body_local_items: Arena<BodyId, Option<BodyLocalItems>>,
    interner: &'crate_data mut NameInterner,
    cancellation: rg_std::CancellationToken,
}

impl<'crate_data> CrateBodyBuildState<'crate_data> {
    pub(super) fn new(
        crate_ref: CrateRef,
        parse_package: &'crate_data rg_parse::Package,
        crate_bodies: LoweredCrateBodies,
        interner: &'crate_data mut NameInterner,
        cancellation: rg_std::CancellationToken,
    ) -> Self {
        let mut body_refs = Arena::with_capacity(crate_bodies.bodies().len());
        let mut body_slots = HashMap::with_capacity(crate_bodies.bodies().len());
        for (body, _) in crate_bodies.bodies().iter_with_ids() {
            let body_ref = BodyRef { crate_ref, body };
            let allocated = body_refs.alloc(body_ref);
            debug_assert_eq!(allocated, body);
            body_slots.insert(body_ref, body);
        }
        Self {
            crate_ref,
            parse_package,
            crate_bodies,
            body_refs,
            body_slots,
            body_facts: Arena::new(),
            body_local_items: Arena::new(),
            interner,
            cancellation,
        }
    }

    /// Start the same semantic pipeline from request-local lowered roots.
    pub(super) fn for_current(
        crate_ref: CrateRef,
        parse_package: &'crate_data rg_parse::Package,
        crate_bodies: LoweredCrateBodies,
        body_refs: Vec<BodyRef>,
        interner: &'crate_data mut NameInterner,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            crate_bodies.bodies().len() == body_refs.len(),
            "current body worklist and identity list have different lengths",
        );
        anyhow::ensure!(
            body_refs.iter().all(|body| body.crate_ref == crate_ref),
            "current body identity belongs to a different crate",
        );
        anyhow::ensure!(
            body_refs.iter().enumerate().all(|(index, body)| {
                !body_refs[..index].iter().any(|previous| previous == body)
            }),
            "current body identity was assigned to more than one root",
        );
        let body_slots = body_refs
            .iter()
            .copied()
            .enumerate()
            .map(|(slot, body_ref)| (body_ref, BodyId(slot)))
            .collect();
        Ok(Self {
            crate_ref,
            parse_package,
            crate_bodies,
            body_refs: Arena::from_vec(body_refs),
            body_slots,
            body_facts: Arena::new(),
            body_local_items: Arena::new(),
            interner,
            cancellation,
        })
    }

    /// Resolve one lowered crate through a visibility-scoped semantic lookup query.
    ///
    /// The crate gets its own use-site visibility and solver answers. Dependency candidate
    /// composition and canonical crate declarations are reused from caches owned by the
    /// surrounding project build.
    pub(super) fn resolve(
        mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_cache: &ItemLookupQueryCache,
    ) -> anyhow::Result<CrateBodies> {
        let span = tracing::debug_span!(
            "body_ir_crate_resolution",
            rg.crate_id = self.crate_ref.crate_id.0,
        );
        let _entered = span.enter();
        let resolution_started = Instant::now();

        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        let phase_started = Instant::now();
        let crate_ref = self.crate_ref;
        self.materialize_body_local_items(
            def_map,
            semantic_ir,
            BodyTaskSource::Saved(self.parse_package),
            |lowered| {
                Ok(BodyRef {
                    crate_ref,
                    body: lowered.body,
                })
            },
        )?;
        let elapsed = phase_started.elapsed();
        let body_local_items_ms = elapsed.as_millis();
        if elapsed >= SLOW_CRATE_RESOLUTION_PHASE {
            tracing::debug!(
                phase = "body_local_items",
                elapsed_ms = elapsed.as_millis(),
                body_count = self.crate_bodies.bodies().len(),
                "slow Body IR crate resolution phase"
            );
        }

        // Build the visibility query before any body query starts. Declaration-local indexes live
        // in Semantic IR; items declared inside a body remain a separate overlay through
        // `BodyBuildQuerySource`.
        let phase_started = Instant::now();
        let crate_items = CrateItemQuery::new(def_map, semantic_ir, self.crate_ref);
        let item_lookup_query =
            ItemLookupQuery::build_with_cache(&crate_items, item_lookup_cache, &self.cancellation)?;
        let elapsed = phase_started.elapsed();
        let item_lookup_query_ms = elapsed.as_millis();
        if elapsed >= SLOW_CRATE_RESOLUTION_PHASE {
            tracing::debug!(
                phase = "item_lookup_query",
                elapsed_ms = elapsed.as_millis(),
                "slow Body IR crate resolution phase"
            );
        }
        let semantic_timings =
            self.resolve_semantics(def_map, semantic_ir, &item_lookup_query, |_| Ok(()))?;
        let body_local_impl_headers_ms = semantic_timings.impl_headers.as_millis();
        let pattern_bindings_ms = semantic_timings.pattern_bindings.as_millis();
        let bodies_ms = semantic_timings.bodies.as_millis();

        // Finalize the build state, e.g. associate each body with its corresponding
        // defmap/item store.
        let body_count = self.crate_bodies.bodies().len();
        let finish_started = Instant::now();
        rg_std::check_cancel!(self.cancellation, "finalize crate bodies");
        let bodies = self.finish();
        let finish_ms = finish_started.elapsed().as_millis();
        tracing::trace!(
            body_count,
            body_local_items_ms,
            item_lookup_query_ms,
            body_local_impl_headers_ms,
            pattern_bindings_ms,
            bodies_ms,
            finish_ms,
            total_ms = resolution_started.elapsed().as_millis(),
            "Body IR crate resolution phases finished"
        );
        Ok(bodies)
    }

    fn body_ref(&self, body: BodyId) -> BodyRef {
        self.body_refs[body]
    }
}
