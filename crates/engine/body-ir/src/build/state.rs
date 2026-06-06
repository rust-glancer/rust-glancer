//! Target-local mutable state used while Body IR resolution is assembled.

use rg_def_map::DefMapReadTxn;
use rg_ir_model::{
    BodyId, BodyRef, ConstRef, DefMapRef, ItemOwner, ModuleRef, StaticRef, TargetRef,
    TypePathResolution,
};
use rg_ir_storage::{ItemLookupIndex, ItemStore, Path, TargetItemQuery};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::NameInterner;

use crate::{
    BodyLocalItems, BodyOwner, TargetBodies,
    resolution::{
        BodyQuerySource, BodyResolver, BodyTypePathResolver, TypeRefUseSite, push_unique,
    },
};

use super::{
    body_def_map::BodyDefMapCollector,
    body_item_store::BodyItemStoreCollector,
    lower::{BodyLoweringTask, BodyTaskLowering},
    query_source::BodyBuildQuerySource,
};

/// Coordinates all body-local facts needed to resolve one target's bodies.
pub(super) struct TargetBodyBuildState<'target> {
    target: TargetRef,
    parse_package: &'target rg_parse::Package,
    target_bodies: &'target mut TargetBodies,
    body_local_items: Vec<Option<BodyLocalItems>>,
    interner: &'target mut NameInterner,
}

impl<'target> TargetBodyBuildState<'target> {
    pub(super) fn new(
        target: TargetRef,
        parse_package: &'target rg_parse::Package,
        target_bodies: &'target mut TargetBodies,
        interner: &'target mut NameInterner,
    ) -> Self {
        Self {
            target,
            parse_package,
            target_bodies,
            body_local_items: Vec::new(),
            interner,
        }
    }

    pub(super) fn resolve(
        mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<()> {
        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        self.materialize_body_local_items(def_map, semantic_ir)?;
        self.resolve_body_local_impl_headers(def_map, semantic_ir)?;

        // Now that we collected body local items, we can build a lookup index
        // to match impls/functions/traits without a complex scan.
        // Note that here we do _not_ include body-local items; these are routed in
        // later via `BodyBuildQuerySource`.
        let target_items = TargetItemQuery::new(def_map, semantic_ir, self.target);
        let semantic_index = ItemLookupIndex::build_from(&target_items)?;

        // Do a pass on resolving body expressions.
        self.resolve_bodies(def_map, semantic_ir, &semantic_index)?;

        // Finalize the build state, e.g. associate each body with its corresponding
        // defmap/item store.
        self.finish();

        Ok(())
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
        // `body_local_items` is the cursor into `target_bodies`: each collected slot means that
        // body has its local DefMap/item store ready. Nested lowering may extend `target_bodies`,
        // so the loop stops only once every appended body has been collected too.
        while self.body_local_items.len() < self.target_bodies.bodies().len() {
            let body_idx = self.body_local_items.len();
            let body_ref = self.body_ref(body_idx);
            let items = self.collect_body_local_items(body_idx, def_map, semantic_ir)?;
            let fallback_module = self.target_bodies.bodies()[body_idx].fallback_module();
            let nested_tasks =
                Self::nested_body_tasks(body_ref, fallback_module, &items.item_store);
            self.body_local_items.push(Some(items));

            if !nested_tasks.is_empty() {
                BodyTaskLowering::new(self.parse_package, self.target_bodies, self.interner)
                    .lower_tasks(&nested_tasks)?;
            }
        }

        Ok(())
    }

    // Collects the local items within a single already-lowered body.
    fn collect_body_local_items(
        &self,
        body_idx: usize,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<BodyLocalItems> {
        let body_ref = self.body_ref(body_idx);
        let body = &self.target_bodies.bodies()[body_idx];

        // Finalization can see previously collected body-local DefMaps. This is what lets nested
        // bodies import names from the body scope that declared them.
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.target, &self.body_local_items);
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
    ) -> anyhow::Result<()> {
        for body_idx in 0..self.target_bodies.bodies().len() {
            let body_ref = self.body_ref(body_idx);
            let body = &self.target_bodies.bodies()[body_idx];
            let resolved_headers = {
                let Some(items) = self.body_local_items.get(body_idx).and_then(Option::as_ref)
                else {
                    continue;
                };
                let impl_headers = items
                    .item_store
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
                    self.target,
                    &self.body_local_items,
                );
                let resolver = BodyTypePathResolver::new(BodyQuerySource::new(
                    &source, &source, body_ref, body,
                ));
                let mut resolved_headers = Vec::new();
                for (impl_id, owner, self_ty, trait_ref) in impl_headers {
                    if owner.origin != DefMapRef::Body(body_ref) {
                        continue;
                    }

                    let Some(scope) = body.scope_for_module(body_ref, owner) else {
                        continue;
                    };

                    let ty = resolver
                        .type_ref(TypeRefUseSite::Scope(scope))
                        .resolve(&self_ty)?;
                    let mut resolved_self_tys = Vec::new();
                    for nominal in ty.as_nominals() {
                        push_unique(&mut resolved_self_tys, nominal.def);
                    }

                    let mut resolved_trait_refs = Vec::new();
                    if let Some(trait_ref) = trait_ref
                        && let Some(path) = Path::from_type_ref(&trait_ref)
                        && let TypePathResolution::Traits(traits) =
                            resolver.resolve_in_scope(scope, &path)?
                    {
                        resolved_trait_refs = traits;
                    }
                    resolved_headers.push((impl_id, resolved_self_tys, resolved_trait_refs));
                }
                resolved_headers
            };

            let Some(items) = self
                .body_local_items
                .get_mut(body_idx)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            for (impl_id, resolved_self_tys, resolved_trait_refs) in resolved_headers {
                if let Some(impl_data) = items.item_store.impls_mut().get_mut(impl_id) {
                    impl_data.resolved_self_tys = resolved_self_tys;
                    impl_data.resolved_trait_refs = resolved_trait_refs;
                }
            }
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
    ) -> anyhow::Result<()> {
        // Make the body resolver aware of body-local items.
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.target, &self.body_local_items);
        let target = self.target;
        let target_bodies = &mut *self.target_bodies;

        for (body_idx, body) in target_bodies.bodies_mut().iter_mut().enumerate() {
            let body_ref = BodyRef {
                target,
                body: BodyId(body_idx),
            };
            BodyResolver::new(&source, &source, semantic_index, body_ref, body).resolve()?;
        }

        Ok(())
    }

    fn finish(mut self) {
        let mut body_local_items = Vec::with_capacity(self.body_local_items.len());
        for body_idx in 0..self.target_bodies.bodies().len() {
            let items = self
                .body_local_items
                .get_mut(body_idx)
                .and_then(Option::take)
                .expect("every built body should have collected body-local items");
            body_local_items.push(items);
        }
        self.target_bodies.set_body_local_items(body_local_items);
    }

    fn body_ref(&self, body_idx: usize) -> BodyRef {
        BodyRef {
            target: self.target,
            body: BodyId(body_idx),
        }
    }
}
