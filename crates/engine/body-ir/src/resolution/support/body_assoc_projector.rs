//! Body-side associated type projection policy.
//!
//! The type layer owns the shared associated type normalizer. Body resolution has one extra source
//! of evidence though: selected impl predicates and body-local callable facts. This facade is the
//! single place that decides the ordinary body projection order:
//!
//! 1. try the body-local impl-predicate bridge;
//! 2. fall back to `rg_ty::TraitSelectionQuery::normalize_assoc_type`.
//!
//! Callers should use this instead of open-coding that order. Keeping the policy here prevents
//! selected-call, selected-method, and nested projection paths from drifting apart.

use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{
    AssocProjectionResult, TraitGoal, TraitSelectionCache, TraitSelectionQuery,
    inference::InferenceTable,
};

use crate::resolution::BodyResolutionContext;

use super::ImplPredicateAssocProjector;

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

    /// Normalize an associated type using the body-local evidence that is safe at this call site.
    ///
    /// This does not solve callable obligations from closure bodies. That path mutates body
    /// inference and remains in the selected-call obligation code. The local bridge here is for
    /// non-callable impl-predicate support such as `S: Source` proving `S::Item`.
    pub(crate) fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, PackageStoreError> {
        if let Some(projection) = ImplPredicateAssocProjector::new(self.context)
            .with_cache(self.trait_selection_cache.clone())
            .project_goal_through_impl_predicates(goal, assoc_name, table)?
        {
            return Ok(Some(projection));
        }

        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        // This fallback can return an inference table to the caller, so it must prove explicit impl
        // where-clauses before making the projection usable.
        .with_cache(self.trait_selection_cache.clone())
        .normalize_assoc_type(goal, assoc_name, table)
    }
}
