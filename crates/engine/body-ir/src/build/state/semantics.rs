//! Resolve impl headers, settle binding identities, and infer facts for each body.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::DefMapRef;
use rg_semantic_ir::{ItemLookupQuery, SemanticIrReadTxn, TypePathResolution};
use rg_std::ExpectedUnique;
use rg_ty::trait_selection::TraitSelectionSession;

use crate::{
    build::{
        pattern_binding::PatternBindingMaterializationPass, query_source::BodyBuildQuerySource,
    },
    resolution::{BodyResolutionContext, InferenceContext},
};

use super::{
    BodySemanticStage, BodySemanticTimings, CrateBodyBuildState, SLOW_CRATE_RESOLUTION_PHASE,
};

// Bodies are the highest-cardinality build unit. Construct their diagnostic record only after the
// timer crosses this threshold; a normal body does not create a tracing span or event.
const SLOW_BODY_RESOLUTION: Duration = Duration::from_secs(1);

impl CrateBodyBuildState<'_> {
    /// Run the semantic stages in the same order for saved and current worklists.
    ///
    /// Lowering and body-local item discovery decide which bodies belong to the worklist. From
    /// this point onward both build modes use exactly these stages: impl headers first, ambiguous
    /// pattern bindings next, then expression facts.
    pub(crate) fn resolve_semantics(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
        mut checkpoint: impl FnMut(BodySemanticStage) -> anyhow::Result<()>,
    ) -> anyhow::Result<BodySemanticTimings> {
        let started = Instant::now();
        self.resolve_body_local_impl_headers(
            def_map,
            semantic_ir,
            item_lookup_query,
            trait_selection,
        )?;
        let impl_headers = started.elapsed();
        Self::report_slow_semantic_stage("body_local_impl_headers", impl_headers, None);
        checkpoint(BodySemanticStage::ImplHeaders)
            .context("check body work after resolving body-local impl headers")?;

        let started = Instant::now();
        self.materialize_pattern_bindings(
            def_map,
            semantic_ir,
            item_lookup_query,
            trait_selection,
        )?;
        let pattern_bindings = started.elapsed();
        Self::report_slow_semantic_stage("pattern_bindings", pattern_bindings, None);
        checkpoint(BodySemanticStage::PatternBindings)
            .context("check body work after resolving pattern bindings")?;

        let started = Instant::now();
        self.resolve_bodies(def_map, semantic_ir, item_lookup_query, trait_selection)?;
        let bodies = started.elapsed();
        Self::report_slow_semantic_stage("bodies", bodies, Some(self.crate_bodies.bodies().len()));
        checkpoint(BodySemanticStage::Bodies).context("check body work after body resolution")?;

        Ok(BodySemanticTimings {
            impl_headers,
            pattern_bindings,
            bodies,
        })
    }

    fn report_slow_semantic_stage(
        phase: &'static str,
        elapsed: Duration,
        body_count: Option<usize>,
    ) {
        if elapsed < SLOW_CRATE_RESOLUTION_PHASE {
            return;
        }
        tracing::debug!(
            phase,
            elapsed_ms = elapsed.as_millis(),
            body_count,
            "slow Body IR crate resolution phase"
        );
    }

    // After body-local item collection, impl headers can be resolved against the body defmap and
    // item store. Both inherent and trait lookups depend on these precomputed header facts.
    fn resolve_body_local_impl_headers(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        for (body_id, lowered_body) in self.crate_bodies.bodies().iter_with_ids() {
            rg_std::check_cancel!(self.cancellation, "body impl headers");
            let body_ref = self.body_ref(body_id);
            let body = lowered_body.body();
            let resolved_headers = {
                let Some(items) = self.body_local_items.get(body_id).and_then(Option::as_ref)
                else {
                    continue;
                };
                let impl_headers = items
                    .item_store()
                    .impls_with_refs()
                    .map(|(impl_ref, impl_data)| {
                        (
                            impl_ref.id,
                            impl_data.owner,
                            impl_data.self_ty.clone(),
                            impl_data.trait_ref.clone(),
                        )
                    })
                    .collect::<Vec<_>>();

                // In order to resolve body-local types, we need to be aware of
                // body-local items, so that's how we inject them.
                let source = BodyBuildQuerySource::new(
                    def_map,
                    semantic_ir,
                    self.crate_ref,
                    &self.body_slots,
                    &self.body_local_items,
                );
                let context = BodyResolutionContext::new(
                    &source,
                    &source,
                    body_ref,
                    body,
                    item_lookup_query,
                    trait_selection.clone(),
                );
                let type_paths = context.type_path_query();
                let mut resolved_headers = Vec::new();
                for (impl_id, owner, self_ty, trait_ref) in impl_headers {
                    rg_std::check_cancel!(self.cancellation, "local impl header");
                    if owner.origin != DefMapRef::Body(body_ref) {
                        continue;
                    }

                    let Some(scope) = body.scope_for_module(body_ref, owner) else {
                        continue;
                    };

                    let ty = context.type_refs(scope).resolve(&self_ty)?;
                    let mut resolved_self_ty = ExpectedUnique::new();
                    for nominal in ty.as_adts() {
                        resolved_self_ty.push(nominal.def);
                    }

                    let mut resolved_trait_ref = ExpectedUnique::new();
                    if let Some(trait_ref) = trait_ref
                        && let Some(path) = trait_ref.as_def_map_path()
                        && let TypePathResolution::Trait(trait_ref) =
                            type_paths.resolve_in_scope(scope, &path)?
                    {
                        resolved_trait_ref.push(trait_ref);
                    }
                    resolved_headers.push((impl_id, resolved_self_ty, resolved_trait_ref));
                }
                resolved_headers
            };

            let Some(items) = self
                .body_local_items
                .get_mut(body_id)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            for (impl_id, resolved_self_ty, resolved_trait_ref) in resolved_headers {
                let _ = items.set_impl_header_facts(impl_id, resolved_self_ty, resolved_trait_ref);
            }
        }

        Ok(())
    }

    fn materialize_pattern_bindings(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );

        for (body_id, body) in self.crate_bodies.bodies_mut().iter_mut_with_ids() {
            rg_std::check_cancel!(self.cancellation, "body pattern bindings");
            let body_ref = self.body_refs[body_id];
            PatternBindingMaterializationPass::new(
                &source,
                &source,
                item_lookup_query,
                body_ref,
                body,
                trait_selection,
            )
            .materialize()?;
        }

        Ok(())
    }

    // For each body with resolved items, infer its expressions and patterns recursively, complete
    // pending semantic operations, and finalize the types and selected declarations.
    fn resolve_bodies(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        // Make body inference aware of body-local items.
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );
        debug_assert!(self.body_facts.is_empty());

        for (body_id, body) in self.crate_bodies.bodies().iter_with_ids() {
            rg_std::check_cancel!(self.cancellation, "body inference");
            let body_ref = self.body_refs[body_id];
            let body = body.body();
            let body_source = body.source();
            let started = Instant::now();
            let facts = InferenceContext::new(
                &source,
                &source,
                item_lookup_query,
                body_ref,
                body,
                trait_selection,
            )
            .infer_body()?;
            let elapsed = started.elapsed();
            if elapsed >= SLOW_BODY_RESOLUTION {
                tracing::debug!(
                    body_id = body_id.0,
                    owner = ?body.owner(),
                    file_id = body_source.file_id.0,
                    path = ?self.parse_package.file_path(body_source.file_id),
                    span = ?body_source.span,
                    elapsed_ms = elapsed.as_millis(),
                    expression_count = body.exprs().len(),
                    binding_count = body.bindings().len(),
                    statement_count = body.statements().len(),
                    "slow body resolution"
                );
            }
            let allocated = self.body_facts.alloc(facts);
            debug_assert_eq!(allocated, body_id);
        }

        Ok(())
    }
}
