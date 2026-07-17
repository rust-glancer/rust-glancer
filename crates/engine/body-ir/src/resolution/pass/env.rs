use rg_def_map::DefMapSource;
use rg_ir_model::BodyRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_ty::TraitSelectionSession;

use crate::{ir::BodyQueryView, resolution::BodyResolutionContext};

/// Stable query inputs retained while a resolution pass derives facts for one immutable body.
///
/// `BodyResolutionContext` borrows both structure and the facts accumulated so far, so it cannot
/// be stored while those facts are being updated. The environment keeps only body-independent
/// inputs and creates a context from each short-lived query view taken by a pass step.
pub(super) struct BodyResolutionEnv<'query, D, I> {
    def_maps: &'query D,
    item_stores: &'query I,
    semantic_index: &'query ItemLookupIndex,
    body_ref: BodyRef,
    trait_selection: &'query TraitSelectionSession,
}

impl<D, I> Clone for BodyResolutionEnv<'_, D, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D, I> Copy for BodyResolutionEnv<'_, D, I> {}

impl<'query, D, I> BodyResolutionEnv<'query, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(super) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        trait_selection: &'query TraitSelectionSession,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
            trait_selection,
        }
    }

    pub(super) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(super) fn context<'source>(
        &'source self,
        body: BodyQueryView<'source>,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        BodyResolutionContext::for_query(
            self.def_maps,
            self.item_stores,
            self.body_ref,
            body,
            self.semantic_index,
            self.trait_selection.clone(),
        )
    }
}
