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
mod callable;

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{
    AssocTypeBinding, Clause, GenericArg, ImplHeader, Substitution, TraitApplication, TraitGoal,
    TraitSelection, TraitSelectionOptions, Ty,
};

use crate::resolution::BodyResolutionContext;

use self::callable::{BodyCallableGoalOutcome, BodyCallableGoalSolver};
use super::BodyInferenceCtx;

/// Solves bounded trait obligations while preserving inference-table semantics.
pub(super) struct BodyTraitObligationSolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// Canonical impl data prepared for evaluation against body-owned facts.
struct BodySelectedImpl {
    header: ImplHeader,
    selection: TraitSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyTraitGoalOutcome {
    /// The selected goal and every recursively exposed predicate were established.
    Solved,
    /// Body-local facts may make the goal solvable on a later fixed-point pass.
    Deferred,
    /// The known receiver shape has no uniquely applicable bounded solution.
    Rejected,
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Probe a trait goal using the crate lookup index persisted with Body IR.
    ///
    /// Keeping this as probe mode matters: callers decide when an `ExpectedUnique::One` result is
    /// strong enough to commit the returned inference table.
    fn probe_trait_goal(
        &self,
        goal: &TraitGoal,
        inference: &mut BodyInferenceCtx,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        self.context
            .trait_selection_with_cache(inference.trait_selection_cache())
            // The returned table may be committed below, so this path uses full predicate solving
            // instead of treating explicit impl where-clauses as someone else's obligation.
            .probe(goal, &inference.table)
    }

    fn evaluate_trait_goals(
        &self,
        inference: &mut BodyInferenceCtx,
        goals: Vec<TraitGoal>,
    ) -> Result<BodyTraitGoalOutcome, PackageStoreError> {
        self.evaluate_trait_goals_inner(inference, goals, &mut Vec::new())
    }

    /// Instantiate every implemented-trait clause and attach equalities for that application.
    ///
    /// Parenthesized `Fn*` syntax follows the same path: its tuple input is positional and its
    /// `Output` equality is an associated binding, so the closure-local solver receives an
    /// ordinary `TraitGoal` rather than a syntax-only special case.
    pub(crate) fn solve_selected_call(
        &self,
        inference: &mut BodyInferenceCtx,
        clauses: &[Clause],
        subst: &rg_ty::inference::InferenceSubstitution,
    ) -> Result<bool, PackageStoreError> {
        let goals = Self::trait_goals_from_clauses(clauses, subst.as_substitution());
        Ok(matches!(
            self.evaluate_trait_goals(inference, goals)?,
            BodyTraitGoalOutcome::Solved
        ))
    }

    fn evaluate_trait_goals_inner(
        &self,
        inference: &mut BodyInferenceCtx,
        goals: Vec<TraitGoal>,
        active: &mut Vec<TraitApplication>,
    ) -> Result<BodyTraitGoalOutcome, PackageStoreError> {
        for goal in goals {
            let outcome = self.evaluate_trait_goal(inference, &goal, active)?;
            if !matches!(outcome, BodyTraitGoalOutcome::Solved) {
                return Ok(outcome);
            }
        }
        Ok(BodyTraitGoalOutcome::Solved)
    }

    /// Instantiate semantic clauses and group projection equalities with their trait goal.
    fn trait_goals_from_clauses(clauses: &[Clause], subst: &Substitution) -> Vec<TraitGoal> {
        let clauses = clauses
            .iter()
            .map(|clause| subst.apply_clause(clause))
            .collect::<Vec<_>>();
        clauses
            .iter()
            .filter_map(|clause| {
                let Clause::Implemented(application) = clause else {
                    return None;
                };
                let associated_types = clauses
                    .iter()
                    .filter_map(|candidate| {
                        let Clause::AliasEq { alias, ty } = candidate else {
                            return None;
                        };
                        (alias.args == application.args).then(|| AssocTypeBinding {
                            associated_ty: alias.associated_ty,
                            ty: ty.clone(),
                        })
                    })
                    .collect();
                Some(TraitGoal {
                    application: application.clone(),
                    associated_types,
                })
            })
            .collect()
    }

    fn evaluate_trait_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
        active: &mut Vec<TraitApplication>,
    ) -> Result<BodyTraitGoalOutcome, PackageStoreError> {
        // Bounds often contain semantic projections, as in
        // `B: FromIterator<<Self as Iterator>::Item>`. Normalize those facts before impl-header
        // matching so the candidate receives concrete evidence rather than an opaque alias shape.
        let goal = self.normalize_trait_goal(inference, goal)?;

        // An associated alias that survived normalization is not usable body evidence. Passing it
        // into the closure-local solver would persist that opaque shape in a binding slot, while
        // passing it into the structural selector could mark a generic impl complete without
        // relating the alias's eventual value to the impl parameters. Leave the whole goal pending
        // so the parent fixed point can retry after the producer makes the alias projectable.
        let has_unresolved_projection = goal
            .application
            .args
            .iter()
            .filter_map(GenericArg::as_ty)
            .any(Ty::has_projection)
            || goal
                .associated_types
                .iter()
                .any(|binding| binding.ty.has_projection());
        if has_unresolved_projection {
            return Ok(BodyTraitGoalOutcome::Deferred);
        }

        // Fn* trait goals can sometimes be answered from a body-local closure witness before the
        // shared trait selector has enough body-specific evidence to prove them.
        match BodyCallableGoalSolver::new(self.context).solve_goal(inference, &goal)? {
            BodyCallableGoalOutcome::Solved => return Ok(BodyTraitGoalOutcome::Solved),
            BodyCallableGoalOutcome::Deferred => return Ok(BodyTraitGoalOutcome::Deferred),
            BodyCallableGoalOutcome::NotApplicable => {}
        }

        // A bare inference slot or declaration parameter has no receiver shape with which to
        // narrow the impl index. Enumerating every visible impl is both expensive and the wrong
        // source of type evidence: Body IR can retry the call obligation after ordinary argument
        // constraints solve the slot, while a generic parameter must be justified by its declared
        // environment rather than by whichever impls happen to be visible.
        let self_ty = inference.root_resolved_ty(goal.self_ty());
        if matches!(self_ty, Ty::InferVar { .. } | Ty::Param(_) | Ty::Unknown) {
            return Ok(BodyTraitGoalOutcome::Deferred);
        }

        let selection = self.probe_trait_goal(&goal, inference)?;
        if let ExpectedUnique::One(selection) = selection {
            inference.table = selection.table;
            return Ok(BodyTraitGoalOutcome::Solved);
        }

        // Chalk cannot see body-owned closure witnesses. If direct proof failed, select one
        // canonical impl header without its predicates and evaluate those predicates here. This
        // handles nested adapters such as `Map<Filter<I, P>, F>` while keeping impl enumeration in
        // `rg_ty` and body-specific evidence in this layer.
        // Selecting the same impl can allocate fresh slots for parameters that occur only in its
        // predicates. Compare the entered goal shapes modulo those allocation IDs; raw equality
        // would see `?0`, `?1`, ... as perpetual progress and recurse forever.
        if active
            .iter()
            .any(|active| active.equivalent_modulo_inference_ids(&goal.application))
        {
            return Ok(BodyTraitGoalOutcome::Deferred);
        }
        active.push(goal.application.clone());
        let result = self.evaluate_selected_impl_predicates(inference, &goal, active);
        active.pop();
        result
    }

    fn evaluate_selected_impl_predicates(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
        active: &mut Vec<TraitApplication>,
    ) -> Result<BodyTraitGoalOutcome, PackageStoreError> {
        let Some(selected) = self.select_impl_for_body(inference, goal)? else {
            return Ok(BodyTraitGoalOutcome::Rejected);
        };

        let mut trial = inference.clone();
        trial.table = selected.selection.table;
        let goals = Self::trait_goals_from_clauses(
            &selected.header.clauses,
            selected.selection.subst.as_substitution(),
        );
        let outcome = self.evaluate_trait_goals_inner(&mut trial, goals, active)?;
        if !matches!(outcome, BodyTraitGoalOutcome::Solved) {
            return Ok(outcome);
        }

        *inference = trial;
        Ok(BodyTraitGoalOutcome::Solved)
    }

    /// Select one canonical impl header and provide inference slots for impl-only type params.
    ///
    /// Header matching binds only parameters visible in `Self` or the trait inputs. Body-owned
    /// evidence can solve the remaining parameters from predicates, so both predicate evaluation
    /// and associated projection must start from the same complete trial substitution.
    fn select_impl_for_body(
        &self,
        inference: &BodyInferenceCtx,
        goal: &TraitGoal,
    ) -> Result<Option<BodySelectedImpl>, PackageStoreError> {
        let selection_query = self
            .context
            .trait_selection_with_cache(inference.trait_selection_cache())
            .with_options(TraitSelectionOptions::new().caller_solves_impl_predicates());
        let ExpectedUnique::One(mut selection) = selection_query.probe(goal, &inference.table)?
        else {
            return Ok(None);
        };
        let impl_ref = selection.trait_impl.impl_ref;
        let Some(header) = self.context.impl_matcher().impl_header(impl_ref)? else {
            return Ok(None);
        };
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Impl(impl_ref))?;

        // Direct header matching only binds parameters that occur in `Self` or the trait inputs.
        // Predicate-only parameters still need trial vars before their clauses can provide facts.
        let matched_subst = selection.subst.clone();
        let mut subst = Substitution::identity(&generics);
        subst.extend(selection.subst.into_substitution());
        for param in generics.iter_self() {
            let GenericParamRef::Type(param) = param.param() else {
                continue;
            };
            if matched_subst.type_param(param).is_none() {
                subst.push(
                    GenericParamRef::Type(param),
                    GenericArg::Type(Box::new(selection.table.new_type_var())),
                );
            }
        }
        selection.subst = rg_ty::inference::InferenceSubstitution::from_substitution(subst);
        Ok(Some(BodySelectedImpl { header, selection }))
    }

    fn normalize_trait_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
    ) -> Result<TraitGoal, PackageStoreError> {
        let mut args = Vec::with_capacity(goal.application.args.len());
        for arg in &goal.application.args {
            args.push(match arg {
                GenericArg::Type(ty) => {
                    GenericArg::Type(Box::new(self.normalize_ty(inference, ty)?))
                }
                GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
            });
        }

        let mut associated_types = Vec::with_capacity(goal.associated_types.len());
        for binding in &goal.associated_types {
            associated_types.push(AssocTypeBinding {
                associated_ty: binding.associated_ty,
                ty: self.normalize_ty(inference, &binding.ty)?,
            });
        }

        Ok(TraitGoal {
            application: TraitApplication {
                def: goal.application.def,
                args: args.into(),
            },
            associated_types,
        })
    }
}
