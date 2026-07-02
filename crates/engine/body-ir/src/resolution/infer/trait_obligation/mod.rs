//! Trait-obligation solving that is allowed to interact with body inference.
//!
//! This layer is intentionally between Body IR and `rg_ty::TraitSelectionQuery`: it understands
//! where bounds were written and can commit inference-table changes, but the actual impl matching
//! still lives in the shared type layer.
//!
//! There are two related flows here:
//!
//! - selected-call obligations, such as a selected method's `where R: From<Self::Output>` bound;
//! - selected-impl associated alias projection, such as projecting `Self::Output` through an impl
//!   whose where-clause contains `F: FnMut(S::Item) -> B`.
//!
//! Both flows need the same body-local trait probing and inference-table commit semantics, so they
//! share this facade. The detailed steps live in child modules so each file can read as one story.

mod assoc_projection;
mod obligation;
mod selected_call;

use rg_ir_model::TraitApplicability;
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    AssocProjectionResult, TraitGoal, TraitSelection, TraitSelectionCache, TraitSelectionOptions,
    TraitSelectionQuery, inference::InferenceTable,
};

use crate::resolution::{BodyResolutionContext, support::ImplPredicateAssocProjector};

use super::BodyCallableGoalSolver;
use super::BodyInferenceCtx;

use self::obligation::{BodyCallableObligation, BodyObligation, BodyObligationGoal};

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
        inference: &mut BodyInferenceCtx,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        // Body obligations are emitted from selected calls and then evaluated by this local
        // fixed-point pass. Treat explicit impl where-clauses as the caller's responsibility here
        // so one body-local obligation does not build a whole Chalk program for a target. Generic
        // parameter bounds still reject the impl in this mode.
        .with_options(TraitSelectionOptions::new().caller_solves_where_predicates())
        .with_cache(inference.trait_selection_cache.clone())
        .probe(goal, &inference.table)
    }

    fn normalize_assoc_type_in_table(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
        cache: &TraitSelectionCache,
    ) -> Result<Option<AssocProjectionResult>, PackageStoreError> {
        // Nested support goals often need the same impl-predicate projection as the outer selected
        // alias. For example, while solving `Adapter<S, F>::Output = B`, a callable predicate may
        // mention `S::Item`, and `S` may itself be another projected impl type. The type-layer
        // normalizer does not inspect those caller-owned support predicates, so try the body-local
        // support path before falling back to the shared query.
        if let Some(projection) = ImplPredicateAssocProjector::new(self.context)
            .project_goal_through_impl_predicates(goal, assoc_name, table)?
        {
            let (ty, table) = projection.into_parts();
            return Ok(Some(AssocProjectionResult {
                ty,
                applicability: TraitApplicability::Yes,
                table,
            }));
        }

        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        .with_options(TraitSelectionOptions::new().caller_solves_where_predicates())
        .with_cache(cache.clone())
        .normalize_assoc_type(goal, assoc_name, table)
    }

    /// Evaluate one body obligation using today's local solver hooks.
    ///
    /// This is deliberately shallow. It preserves the current policy of applying closure-callable
    /// evidence first, then probing visible trait impls and committing only a unique trial table.
    fn evaluate_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        obligation: BodyObligation,
    ) -> Result<(), PackageStoreError> {
        match obligation.into_goal() {
            BodyObligationGoal::Trait(goal) => self.evaluate_trait_goal(inference, &goal),
            BodyObligationGoal::Callable(goal) => {
                self.evaluate_callable_obligation(inference, &goal)
            }
        }
    }

    fn evaluate_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        obligations: Vec<BodyObligation>,
    ) -> Result<(), PackageStoreError> {
        for obligation in obligations {
            self.evaluate_obligation(inference, obligation)?;
        }
        Ok(())
    }

    fn evaluate_trait_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
    ) -> Result<(), PackageStoreError> {
        // Fn* trait goals can sometimes be answered from a body-local closure witness before the
        // shallow trait selector has enough type-system machinery to prove them.
        if BodyCallableGoalSolver::new(self.context).solve_goal(inference, goal)? {
            return Ok(());
        }

        let selection = self.probe_trait_goal(goal, inference)?;
        if let ExpectedUnique::One(selection) = selection {
            inference.table = selection.table;
        }

        Ok(())
    }

    fn evaluate_callable_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        obligation: &BodyCallableObligation,
    ) -> Result<(), PackageStoreError> {
        // Callable obligations are best-effort closure evidence. If this local hook cannot learn
        // anything from the closure body, there is no separate trait-selection fallback in this
        // path: the obligation has already been classified as callable-only evidence.
        let _solved = BodyCallableGoalSolver::new(self.context).solve_fn_trait_goal(
            inference,
            obligation.self_ty(),
            obligation.params(),
            obligation.ret(),
        )?;
        Ok(())
    }
}
