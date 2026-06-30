//! Trait-obligation solving that is allowed to interact with body inference.
//!
//! This layer is intentionally between Body IR and `rg_ty::TraitSelectionQuery`: it understands
//! where bounds were written and can commit inference-table changes, but the actual impl matching
//! still lives in the shared type layer.
//!
//! There are two related flows here:
//!
//! - selected-call obligations, such as `where B: FromIterator<Self::Item>` on a selected method;
//! - selected-impl associated alias projection, such as projecting `Self::Item` through an impl
//!   whose where-clause contains `F: FnMut(S::Item) -> B`.
//!
//! Both flows need the same body-local trait probing and inference-table commit semantics, so they
//! share this facade. The detailed steps live in child modules so each file can read as one story.

mod assoc_projection;
mod selected_call;

use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{TraitGoal, TraitSelection, TraitSelectionQuery, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

use super::BodyInferenceCtx;

pub(super) use selected_call::SelectedCallObligationInput;

/// Solves bounded trait obligations while preserving inference-table semantics.
pub(super) struct BodyTraitObligationSolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Probe a trait goal using the target lookup index persisted with Body IR.
    ///
    /// Keeping this as probe mode matters: callers decide when an `ExpectedUnique::One` result is
    /// strong enough to commit the returned inference table.
    fn probe_trait_goal(
        &self,
        goal: &TraitGoal,
        inference: &BodyInferenceCtx,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        self.probe_trait_goal_in_table(goal, &inference.table)
    }

    fn probe_trait_goal_in_table(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        .probe(goal, table)
    }
}
