//! Body-side access to shared semantic type normalization.
//!
//! This facade handles projections that need no body-local evidence. Body inference layers its
//! closure-aware canonical-clause evaluation on top, while ordinary queries can stay independent
//! of mutable body inference state.

use rg_def_map::DefMapSource;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{TraitSelectionCache, TraitSelectionQuery, Ty, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

pub(crate) struct BodyAssocProjector<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    trait_selection_cache: TraitSelectionCache,
}

impl<'query, D, I> BodyAssocProjector<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self {
            context,
            trait_selection_cache: TraitSelectionCache::default(),
        }
    }

    pub(crate) fn with_cache(mut self, cache: TraitSelectionCache) -> Self {
        self.trait_selection_cache = cache;
        self
    }

    /// Normalize projections anywhere inside one semantic type.
    pub(crate) fn normalize_ty(
        &self,
        ty: &Ty,
        table: &InferenceTable,
    ) -> Result<(Ty, InferenceTable), PackageStoreError> {
        self.query().normalize_ty(ty, table)
    }

    fn query(
        &self,
    ) -> TraitSelectionQuery<
        'query,
        crate::resolution::BodyQuerySource<'query, D, I>,
        crate::resolution::BodyQuerySource<'query, D, I>,
    > {
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.crate_items(),
            self.context.semantic_index(),
        )
        .with_cache(self.trait_selection_cache.clone())
    }
}
