use rg_ir_model::BodyRef;
use rg_semantic_ir::ItemLookupIndex;
use rg_ty::TraitSelectionCache;

use crate::{ir::body::ResolvedBodyData, resolution::BodyResolutionContext};

/// Stable query inputs retained while a resolution pass mutates its body.
///
/// `BodyResolutionContext` borrows the body it queries, so it cannot be stored beside the pass's
/// mutable body reference. The environment keeps only the body-independent inputs and creates a
/// context from each short-lived shared borrow taken by a pass step.
pub(super) struct BodyResolutionEnv<'query, D, I> {
    def_maps: &'query D,
    item_stores: &'query I,
    semantic_index: &'query ItemLookupIndex,
    body_ref: BodyRef,
    trait_selection_cache: &'query TraitSelectionCache,
}

impl<D, I> Clone for BodyResolutionEnv<'_, D, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D, I> Copy for BodyResolutionEnv<'_, D, I> {}

impl<'query, D, I> BodyResolutionEnv<'query, D, I> {
    pub(super) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        trait_selection_cache: &'query TraitSelectionCache,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
            trait_selection_cache,
        }
    }

    pub(super) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(super) fn context<'source>(
        &'source self,
        body: &'source ResolvedBodyData,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        BodyResolutionContext::with_trait_selection_cache(
            self.def_maps,
            self.item_stores,
            self.body_ref,
            body,
            self.semantic_index,
            self.trait_selection_cache,
        )
    }
}
