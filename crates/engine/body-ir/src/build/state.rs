//! Target-local mutable state used while Body IR resolution is assembled.

use rg_def_map::DefMapReadTxn;
use rg_ir_model::{BodyId, BodyRef, DefMapRef, ScopeId, TargetRef};
use rg_ir_storage::{DefMap, ItemLookupIndex, ItemStore, TargetItemQuery};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;

use crate::{
    TargetBodies,
    ir::body_map::{BodyDefMapCollector, BodyItemStoreCollector},
    resolution::{BodyQuerySource, BodyResolver, BodyTypePathResolver, push_unique},
};

use super::query_source::BodyBuildQuerySource;

/// Body-local item facts collected for one lowered body.
pub(super) struct BodyLocalItems {
    pub(super) def_map: DefMap,
    pub(super) item_store: ItemStore,
}

/// Coordinates all body-local facts needed to resolve one target's bodies.
pub(super) struct TargetBodyBuildState<'target> {
    target: TargetRef,
    target_bodies: &'target mut TargetBodies,
    body_local_items: Vec<Option<BodyLocalItems>>,
}

impl<'target> TargetBodyBuildState<'target> {
    pub(super) fn new(target: TargetRef, target_bodies: &'target mut TargetBodies) -> Self {
        Self {
            target,
            target_bodies,
            body_local_items: Vec::new(),
        }
    }

    pub(super) fn resolve(
        mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> Result<(), PackageStoreError> {
        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        self.collect_body_local_items();
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

    // Go through each body, and collect definitions & items within this body.
    fn collect_body_local_items(&mut self) {
        self.body_local_items.clear();
        for (body_idx, body) in self.target_bodies.bodies().iter().enumerate() {
            let body_ref = self.body_ref(body_idx);

            // Body-local item collection is separated from expression resolution so future passes
            // can finalize imports and discover nested body owners before any body is resolved.
            let def_map = BodyDefMapCollector::new(body_ref, body).collect();
            let item_store = BodyItemStoreCollector::new(body, &def_map).collect();
            self.body_local_items.push(Some(BodyLocalItems {
                def_map,
                item_store,
            }));
        }
    }

    // After we collected body local items, we need to resolve each `impl` block's `Self`
    // type to its nominal candidates.
    fn resolve_body_local_impl_headers(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> Result<(), PackageStoreError> {
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
                        (impl_ref.id, impl_data.owner, impl_data.self_ty.clone())
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
                for (impl_id, owner, self_ty) in impl_headers {
                    if owner.origin != DefMapRef::Body(body_ref) {
                        continue;
                    }

                    // TODO: We should probably avoid such a direct conversion,
                    // maybe implement a dedicated method at least. Right now, this is an
                    // implicit invariant.
                    // This works because we create synthetic modules as well, and these
                    // are allocated in `ScopeId` order (so conversion is only safe if
                    // `body.scope(scope).is_some()`). Better to make it explicit and
                    // encapsulated.
                    let scope = ScopeId(owner.module.0);
                    if body.scope(scope).is_none() {
                        continue;
                    }

                    let ty = resolver.resolve_type_ref_in_scope(&self_ty, scope)?;
                    let mut resolved_self_tys = Vec::new();
                    for nominal in ty.as_nominals() {
                        push_unique(&mut resolved_self_tys, nominal.def);
                    }
                    resolved_headers.push((impl_id, resolved_self_tys));
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
            for (impl_id, resolved_self_tys) in resolved_headers {
                if let Some(impl_data) = items.item_store.impls_mut().get_mut(impl_id) {
                    impl_data.resolved_self_tys = resolved_self_tys;
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
    ) -> Result<(), PackageStoreError> {
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
        for (body_idx, body) in self.target_bodies.bodies_mut().iter_mut().enumerate() {
            let Some(items) = self
                .body_local_items
                .get_mut(body_idx)
                .and_then(Option::take)
            else {
                continue;
            };

            body.body_def_map = Some(items.def_map);
            body.body_item_store = Some(items.item_store);
        }
    }

    fn body_ref(&self, body_idx: usize) -> BodyRef {
        BodyRef {
            target: self.target,
            body: BodyId(body_idx),
        }
    }
}
