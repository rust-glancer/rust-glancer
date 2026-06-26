use rg_ir_model::BodyRef;
use rg_ir_storage::ItemLookupIndex;

use crate::ir::body::ResolvedBodyData;

use super::BodyResolutionContext;

/// External stores and indexes shared by body-resolution passes.
///
/// Mutating passes own the active body, but they should not each remember how to thread DefMap,
/// item-store, semantic-index, and body identity into read-only queries. This provider bundle keeps
/// those inputs together and creates short-lived query contexts for whichever body view a pass has.
pub(crate) struct BodyResolutionProviders<'query, D, I> {
    def_maps: &'query D,
    item_stores: &'query I,
    semantic_index: &'query ItemLookupIndex,
    body_ref: BodyRef,
}

impl<D, I> Clone for BodyResolutionProviders<'_, D, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D, I> Copy for BodyResolutionProviders<'_, D, I> {}

impl<'query, D, I> BodyResolutionProviders<'query, D, I> {
    pub(crate) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(crate) fn context<'source>(
        &'source self,
        body: &'source ResolvedBodyData,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        BodyResolutionContext::new(
            self.def_maps,
            self.item_stores,
            self.body_ref,
            body,
            self.semantic_index,
        )
    }
}
