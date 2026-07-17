//! Crate-local mutable state used while Body IR resolution is assembled.

use anyhow::Context as _;
use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{
    BodyId, BodyRef, ConstRef, CrateRef, DefMapRef, ItemOwner, ModuleRef, Path, StaticRef,
    TypePathResolution,
};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStore};
use rg_std::ExpectedUnique;
use rg_text::NameInterner;
use rg_ty::TraitSelectionSession;

use crate::{
    BodyFacts, BodyLocalItems, BodyOwner, CrateBodies,
    resolution::{BodyResolutionContext, BodyResolutionPass},
};

use super::{
    body_def_map::BodyDefMapCollector,
    body_item_store::BodyItemStoreCollector,
    lower::{BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, LoweredCrateBodies},
    pattern_binding::PatternBindingMaterializationPass,
    query_source::BodyBuildQuerySource,
};

/// Coordinates all body-local facts needed to resolve one crate's bodies.
pub(super) struct CrateBodyBuildState<'crate_data> {
    crate_ref: CrateRef,
    parse_package: &'crate_data rg_parse::Package,
    crate_bodies: LoweredCrateBodies,
    body_facts: Arena<BodyId, BodyFacts>,
    body_local_items: Arena<BodyId, Option<BodyLocalItems>>,
    interner: &'crate_data mut NameInterner,
}

impl<'crate_data> CrateBodyBuildState<'crate_data> {
    pub(super) fn new(
        crate_ref: CrateRef,
        parse_package: &'crate_data rg_parse::Package,
        crate_bodies: LoweredCrateBodies,
        interner: &'crate_data mut NameInterner,
    ) -> Self {
        Self {
            crate_ref,
            parse_package,
            crate_bodies,
            body_facts: Arena::new(),
            body_local_items: Arena::new(),
            interner,
        }
    }

    pub(super) fn resolve(
        mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<CrateBodies> {
        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        self.materialize_body_local_items(def_map, semantic_ir)?;

        // Build a crate-semantic lookup index once and persist it with Body IR. Body-local items
        // are deliberately overlaid through `BodyBuildQuerySource` instead of being part of this
        // crate-scoped cache.
        let crate_items = CrateItemQuery::new(def_map, semantic_ir, self.crate_ref);
        let semantic_index = ItemLookupIndex::build_from(&crate_items)?;
        let trait_selection = TraitSelectionSession::new(self.crate_ref);
        self.resolve_body_local_impl_headers(
            def_map,
            semantic_ir,
            &semantic_index,
            &trait_selection,
        )?;

        // Identifier patterns are the last source ambiguity in structural Body IR. Resolve and
        // compact them before the ordinary body pass receives its immutable `BodyData`.
        self.materialize_pattern_bindings(def_map, semantic_ir, &semantic_index, &trait_selection)?;

        // Do a pass on resolving body expressions.
        self.resolve_bodies(def_map, semantic_ir, &semantic_index, &trait_selection)?;

        // Finalize the build state, e.g. associate each body with its corresponding
        // defmap/item store.
        Ok(self.finish(semantic_index))
    }

    // Walk every known body, collecting local facts and lowering newly discovered nested bodies.
    // This is a worklist rather than recursive descent: collecting one body can append nested
    // fn/const/static bodies, and the loop visits those appended bodies before resolution starts.
    fn materialize_body_local_items(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<()> {
        self.body_local_items.clear();
        // `body_local_items` is the cursor into `crate_bodies`: each collected slot means that
        // body has its local DefMap/item store ready. Nested lowering may extend `crate_bodies`,
        // so the loop stops only once every appended body has been collected too.
        let cargo_target = def_map
            .package(self.crate_ref.package)?
            .crate_data(self.crate_ref.crate_id)
            .context("semantic crate should have definition data")?
            .cargo_target();
        let parse_target = self.parse_package.target(cargo_target).with_context(|| {
            format!(
                "while attempting to fetch parsed target {:?} for nested body lowering",
                self.crate_ref.crate_id,
            )
        })?;
        let cfg = CfgEvaluator::new(
            self.parse_package.cfg_options(),
            parse_target.enables_test_cfg(),
        );
        let mut macro_expansion = BodyMacroExpansion::new(self.parse_package, def_map, cfg);
        while self.body_local_items.len() < self.crate_bodies.bodies().len() {
            let body = self.body_local_items.next_id();
            let body_ref = self.body_ref(body);
            let items = self.collect_body_local_items(body, def_map, semantic_ir)?;
            let fallback_module = self.crate_bodies.bodies()[body].body().fallback_module();
            let nested_tasks =
                Self::nested_body_tasks(body_ref, fallback_module, items.item_store());
            let allocated = self.body_local_items.alloc(Some(items));
            debug_assert_eq!(allocated, body);

            if !nested_tasks.is_empty() {
                BodyTaskLowering::new(
                    self.parse_package,
                    &mut self.crate_bodies,
                    cfg,
                    self.interner,
                )
                .lower_tasks(&nested_tasks, &mut macro_expansion)?;
            }
        }

        Ok(())
    }

    // Collects the local items within a single already-lowered body.
    fn collect_body_local_items(
        &self,
        body: BodyId,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<BodyLocalItems> {
        let body_ref = self.body_ref(body);
        let body = self.crate_bodies.bodies()[body].body();

        // Finalization can see previously collected body-local DefMaps. This is what lets nested
        // bodies import names from the body scope that declared them.
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.crate_ref, &self.body_local_items);
        let def_map = BodyDefMapCollector::new(body_ref, body)
            .collect()
            .finalize(source)?;
        let item_store = BodyItemStoreCollector::new(body, &def_map).collect();

        Ok(BodyLocalItems::new(def_map, item_store))
    }

    fn nested_body_tasks(
        body_ref: BodyRef,
        fallback_module: ModuleRef,
        item_store: &ItemStore,
    ) -> Vec<BodyLoweringTask> {
        let origin = DefMapRef::Body(body_ref);
        let mut tasks = Vec::new();

        // Associated items share the function/const arenas with module items. Their body still
        // belongs to the associated item, but type lookup starts from the owning impl/trait module.
        for (function_ref, function_data) in item_store.functions_with_refs() {
            if function_ref.origin != origin {
                continue;
            }
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, function_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Function(function_ref),
                owner_module,
                fallback_module,
                file_id: function_data.source.file_id,
                span: function_data.span,
            });
        }

        for (const_id, const_data) in item_store.consts().iter_with_ids() {
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, const_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Const(ConstRef {
                    origin,
                    id: const_id,
                }),
                owner_module,
                fallback_module,
                file_id: const_data.source.file_id,
                span: const_data.span,
            });
        }

        for (static_id, static_data) in item_store.statics().iter_with_ids() {
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Static(StaticRef {
                    origin,
                    id: static_id,
                }),
                owner_module: static_data.owner,
                fallback_module,
                file_id: static_data.source.file_id,
                span: static_data.span,
            });
        }

        tasks.sort_by_key(|task| (task.file_id.0, task.span.text.start, task.span.text.end));
        tasks
    }

    fn owner_module_for_body_item_owner(
        item_store: &ItemStore,
        owner: ItemOwner,
    ) -> Option<ModuleRef> {
        match owner {
            ItemOwner::Module(module) => Some(module),
            ItemOwner::Trait(trait_id) => item_store.trait_data(trait_id).map(|data| data.owner),
            ItemOwner::Impl(impl_id) => item_store.impl_data(impl_id).map(|data| data.owner),
        }
    }

    // After body-local item collection, impl headers can be resolved against the body defmap and
    // item store. Both inherent and trait lookups depend on these precomputed header facts.
    fn resolve_body_local_impl_headers(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        semantic_index: &ItemLookupIndex,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        for (body_id, lowered_body) in self.crate_bodies.bodies().iter_with_ids() {
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
                    &self.body_local_items,
                );
                let context = BodyResolutionContext::for_structure(
                    &source,
                    &source,
                    body_ref,
                    body,
                    semantic_index,
                    trait_selection.clone(),
                );
                let type_paths = context.type_path_query();
                let mut resolved_headers = Vec::new();
                for (impl_id, owner, self_ty, trait_ref) in impl_headers {
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
                        && let Some(path) = Path::from_type_ref(&trait_ref)
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
        semantic_index: &ItemLookupIndex,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.crate_ref, &self.body_local_items);
        let crate_ref = self.crate_ref;

        for (body_id, body) in self.crate_bodies.bodies_mut().iter_mut_with_ids() {
            let body_ref = BodyRef {
                crate_ref,
                body: body_id,
            };
            PatternBindingMaterializationPass::new(
                &source,
                &source,
                semantic_index,
                body_ref,
                body,
                trait_selection,
            )
            .materialize()?;
        }

        Ok(())
    }

    // For each body with resolved items, goes through the body content and finalizes the resolution,
    // e.g. resolves all the bindings and runs a fixed-point loop until no more information can be
    // extracted.
    fn resolve_bodies(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        semantic_index: &ItemLookupIndex,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        // Make the body resolution pass aware of body-local items.
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.crate_ref, &self.body_local_items);
        let crate_ref = self.crate_ref;
        debug_assert!(self.body_facts.is_empty());

        for (body_id, body) in self.crate_bodies.bodies().iter_with_ids() {
            let body_ref = BodyRef {
                crate_ref,
                body: body_id,
            };
            let facts = BodyResolutionPass::new(
                &source,
                &source,
                semantic_index,
                body_ref,
                body.body(),
                trait_selection,
            )
            .resolve()?;
            let allocated = self.body_facts.alloc(facts);
            debug_assert_eq!(allocated, body_id);
        }

        Ok(())
    }

    fn finish(mut self, semantic_index: ItemLookupIndex) -> CrateBodies {
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

        CrateBodies::from_build(
            coverage,
            semantic_index,
            bodies,
            self.body_facts,
            body_local_items,
        )
    }

    fn body_ref(&self, body: BodyId) -> BodyRef {
        BodyRef {
            crate_ref: self.crate_ref,
            body,
        }
    }
}
