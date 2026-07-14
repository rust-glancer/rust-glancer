//! Trait goals exposed by an already-selected canonical call signature.
//!
//! Signature lowering has already resolved parameter identities, trait applications, and
//! associated type equalities. Body inference only instantiates those facts and submits them to
//! the body-aware evaluator; it must not reinterpret the declaration's `TypeRef` syntax.

use rg_def_map::DefMapSource;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{Clause, inference::InferenceSubstitution};

use super::super::BodyInferenceCtx;
use super::{BodyTraitGoalOutcome, BodyTraitObligationSolver};

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Instantiate every implemented-trait clause and attach equalities for that application.
    ///
    /// Parenthesized `Fn*` syntax follows the same path: its tuple input is positional and its
    /// `Output` equality is an associated binding, so the closure-local solver receives an
    /// ordinary `TraitGoal` rather than a syntax-only special case.
    pub(crate) fn solve_selected_call(
        &self,
        inference: &mut BodyInferenceCtx,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
    ) -> Result<bool, PackageStoreError> {
        let goals = Self::trait_goals_from_clauses(clauses, subst.as_substitution());
        Ok(matches!(
            self.evaluate_trait_goals(inference, goals)?,
            BodyTraitGoalOutcome::Solved
        ))
    }
}
