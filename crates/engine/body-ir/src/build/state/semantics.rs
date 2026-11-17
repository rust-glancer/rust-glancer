//! Resolve local impl headers, then settle binding identities.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rg_def_map::DefinitionReader;
use rg_ir_model::DefMapRef;
use rg_semantic_ir::{ItemLookupQuery, ItemReader, TypePathResolution};
use rg_std::ExpectedUnique;

use super::{BodySemanticStage, BodySemanticTimings, BodyStoreBuilder, SLOW_PREPARATION_PHASE};
use crate::{
    build::{pattern_binding::PatternBindingQuery, query_source::BodyBuildQuerySource},
    resolution::BodyResolutionContext,
};

impl BodyStoreBuilder<'_> {
    /// Run the semantic stages in the same order for saved and current worklists.
    ///
    /// Lowering and body-local item discovery decide which bodies belong to the worklist. From
    /// this point onward both build modes resolve impl headers and ambiguous pattern bindings.
    /// Inference and initializer evaluation only see the frozen result.
    pub(crate) fn prepare_semantics(
        &mut self,
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        mut checkpoint: impl FnMut(BodySemanticStage) -> anyhow::Result<()>,
    ) -> anyhow::Result<BodySemanticTimings> {
        let started = Instant::now();
        self.resolve_body_local_impl_headers(def_map, semantic_ir, item_lookup_query)?;
        let impl_headers = started.elapsed();
        Self::report_slow_semantic_stage("body_local_impl_headers", impl_headers);
        checkpoint(BodySemanticStage::ImplHeaders)
            .context("check body work after resolving body-local impl headers")?;

        let started = Instant::now();
        self.materialize_pattern_bindings(def_map, semantic_ir, item_lookup_query)?;
        let pattern_bindings = started.elapsed();
        Self::report_slow_semantic_stage("pattern_bindings", pattern_bindings);
        checkpoint(BodySemanticStage::PatternBindings)
            .context("check body work after resolving pattern bindings")?;

        Ok(BodySemanticTimings {
            impl_headers,
            pattern_bindings,
        })
    }

    fn report_slow_semantic_stage(phase: &'static str, elapsed: Duration) {
        if elapsed < SLOW_PREPARATION_PHASE {
            return;
        }
        tracing::debug!(
            phase,
            elapsed_ms = elapsed.as_millis(),
            "slow Body IR preparation phase"
        );
    }

    // After body-local item collection, impl headers can be resolved against the body defmap and
    // item store. Both inherent and trait lookups depend on these precomputed header facts.
    fn resolve_body_local_impl_headers(
        &mut self,
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
    ) -> anyhow::Result<()> {
        for (body_id, lowered_body) in self.lowered_bodies.bodies().iter_with_ids() {
            self.source.check_cancelled()?;
            rg_std::check_cancel!(self.cancellation, "body impl headers");
            let body_ref = self.body_ref(body_id);
            let body = lowered_body.body();
            let resolved_headers = {
                let Some(items) = self.body_local_items.get(body_id) else {
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
                    self.cancellation.clone(),
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

            let Some(items) = self.body_local_items.get_mut(body_id) else {
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
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
    ) -> anyhow::Result<()> {
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );

        for (body_id, body) in self.lowered_bodies.bodies_mut().iter_mut_with_ids() {
            rg_std::check_cancel!(self.cancellation, "body pattern bindings");
            let body_ref = self.body_refs[body_id];
            if !body.has_pending_bindings() {
                continue;
            }
            let active = PatternBindingQuery::new(
                &source,
                &source,
                item_lookup_query,
                body_ref,
                body,
                &self.cancellation,
            )
            .active_bindings()
            .context("resolve pattern binding identities")?;
            body.compact_bindings(&active);
        }

        Ok(())
    }
}
