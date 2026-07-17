//! Query-facing Chalk solver state.
//!
//! `TraitSelectionQuery` sends canonical project goals through this module. It makes sure the
//! goal's semantic definitions exist in the shared Chalk program, runs the appropriate solver
//! forest, and translates projection evidence back into rust-glancer inference facts.
//!
//! The state is serialized behind one mutex because a Chalk solver forest mutates as it records
//! answers. Impl-predicate checks and associated-type projection use different forests: the first
//! only needs one answer, while the second must retain the substitution for its result type.

use std::cell::Cell;
use std::sync::Mutex;
use std::time::Instant;

use chalk_engine::solve::SLGSolver;
use chalk_ir::cast::Cast;
use chalk_ir::{
    Binders, Canonical, ConstrainedSubst, DomainGoal, GenericArgData, GoalData, Normalize,
    QuantifierKind,
};
use chalk_solve::Solver;
use chalk_solve::ext::GoalExt;
use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ImplRef, TraitApplicability};
use rg_semantic_ir::{CrateItemQuery, ItemStoreSource};

use super::interner::RgChalkInterner;
use super::lower::{ChalkLowerer, GenericBinderEnv};
use super::program::ChalkProgramState;
use super::projection::{ProjectionAliasLowering, ProjectionAnswerVars};
use super::raise;
use crate::inference::{InferVarKind, InferenceSubstitution, InferenceTable};
use crate::trait_selection::{AssocProjectionResult, TraitSelectionSession};
use crate::{Clause, GenericArgs, ItemPathQuery};

const INTER: RgChalkInterner = RgChalkInterner;
const SOLVER_MAX_SIZE: usize = 32;
const SOLVER_QUANTUM_BUDGET: usize = 4_096;
const OPEN_PROJECTION_QUANTUM_BUDGET: usize = 256;

/// Result of crossing the bounded Chalk adapter.
///
/// `Ambiguous(None)` is useful for proof-only goals, where ambiguity itself is the result.
/// Projection can carry definite guidance inside `Ambiguous(Some(_))`. Unsupported lowering and
/// exhausted work are kept distinct from a real proof failure so the adapter never silently turns
/// an implementation limit into Rust semantics.
#[derive(Debug)]
pub(crate) enum ChalkOutcome<T> {
    Proven(T),
    Ambiguous(Option<T>),
    NoSolution,
    Unsupported,
    Exhausted,
}

/// Deterministic work boundary passed to Chalk's resumable SLG forest.
///
/// Chalk invokes the callback between solver quanta. A forest may reuse completed answers on a
/// later query, but one rust-glancer query is not allowed to monopolize the semantic execution lane
/// indefinitely.
struct SolverBudget {
    remaining: Cell<usize>,
    exhausted: Cell<bool>,
}

enum ProjectionAnswerFailure {
    Unsupported,
    Conflicting,
}

impl SolverBudget {
    fn new(remaining: usize) -> Self {
        Self {
            remaining: Cell::new(remaining),
            exhausted: Cell::new(false),
        }
    }

    fn should_continue(&self) -> bool {
        let remaining = self.remaining.get();
        if remaining == 0 {
            self.exhausted.set(true);
            return false;
        }
        self.remaining.set(remaining - 1);
        true
    }

    fn exhausted(&self) -> bool {
        self.exhausted.get()
    }
}

/// Long-lived Chalk state owned by one `TraitSelectionSession`.
///
/// The semantic program grows as new goals mention new traits. Chalk's solver forests stay beside
/// it so repeated obligations can reuse answers instead of rebuilding a database and solving from
/// scratch for each body.
pub(crate) struct ChalkTraitSolver {
    state: Mutex<ChalkSolverState>,
}

struct ChalkSolverState {
    program: ChalkProgramState,
    impl_bounds_solver: SLGSolver<RgChalkInterner>,
    assoc_projection_solver: SLGSolver<RgChalkInterner>,
}

impl ChalkTraitSolver {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(ChalkSolverState::new()),
        }
    }

    /// Check the predicates of one already-selected impl after applying its inferred arguments.
    ///
    /// Body-local closures are reported as unsupported because their callable signature lives in
    /// Body IR rather than the shared program.
    pub(crate) fn impl_bounds_applicability<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        session: &TraitSelectionSession,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<()>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let instantiated_clauses = clauses
            .iter()
            .map(|clause| subst.as_substitution().apply_clause(clause))
            .collect::<Vec<_>>();

        // Closure identities belong to one body and the Chalk database deliberately has no access
        // to their inferred inputs or output. Let the body obligation solver evaluate these
        // clauses from its closure witness instead of asking Chalk to reason from the stub datum.
        let has_body_local_closure = instantiated_clauses.iter().any(|clause| match clause {
            Clause::Implemented(application) => application
                .args
                .iter()
                .any(|arg| arg.as_ty().is_some_and(crate::Ty::has_closure)),
            Clause::AliasEq { alias, ty } => {
                ty.has_closure()
                    || alias
                        .args
                        .iter()
                        .any(|arg| arg.as_ty().is_some_and(crate::Ty::has_closure))
            }
        });
        if has_body_local_closure {
            return Ok(ChalkOutcome::Unsupported);
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        state.program.ensure_for_clauses(
            item_paths,
            crate_items,
            session,
            &instantiated_clauses,
            Some(table),
        )?;
        Ok(state.impl_bounds_applicability(clauses, subst, table))
    }

    /// Load definitions referenced by visible impl predicates before candidate evaluation begins.
    ///
    /// Candidate selection checks impls one at a time. Priming the program in one pass keeps
    /// semantic program extension outside that repeated solve loop.
    pub(crate) fn prepare_clauses<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        session: &TraitSelectionSession,
        clauses: &[Clause],
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned")
            .program
            .ensure_for_clauses(item_paths, crate_items, session, clauses, None)
    }

    /// Normalize one associated type and return any inference evidence carried by Chalk's answer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        session: &TraitSelectionSession,
        goal: &crate::trait_selection::TraitGoal,
        assoc_name: &str,
        selected_impl: Option<(ImplRef, &InferenceSubstitution)>,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<AssocProjectionResult>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // Chalk only sees a placeholder datum for body-local closures. In particular, it cannot
        // project a closure's `Fn*::Output`; Body IR retries this projection with the closure's
        // actual body-owned inference facts.
        if goal
            .application
            .args
            .iter()
            .any(|arg| arg.as_ty().is_some_and(crate::Ty::has_closure))
            || goal
                .associated_types
                .iter()
                .any(|binding| binding.ty.has_closure())
        {
            return Ok(ChalkOutcome::Unsupported);
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        state
            .program
            .ensure_for_goal(item_paths, crate_items, session, goal, table)?;
        let selected_impl = if let Some((impl_ref, subst)) = selected_impl {
            let generics = item_paths
                .generics()
                .generics(GenericDefRef::Impl(impl_ref))?;
            Some((impl_ref, subst.as_substitution().args_for(&generics)))
        } else {
            None
        };
        Ok(state.normalize_assoc_type(goal, assoc_name, selected_impl.as_ref(), table))
    }
}

impl ChalkSolverState {
    fn new() -> Self {
        Self {
            program: ChalkProgramState::new(),
            // Impl-bound checks and associated projection use separate forests because their goal
            // shapes and answer decoding differ. `expected_answers` is a diagnostic assertion in
            // Chalk, not a work limit; rust-glancer bounds work through `solve_limited` instead.
            impl_bounds_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
            assoc_projection_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
        }
    }

    fn impl_bounds_applicability(
        &mut self,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> ChalkOutcome<()> {
        let binders = GenericBinderEnv::empty();
        let lowerer =
            ChalkLowerer::new(&binders).with_associated_tys(self.program.associated_tys());
        let Some(goals) = lowerer.candidate_where_goals(clauses, subst, table) else {
            return ChalkOutcome::Unsupported;
        };
        if goals.is_empty() {
            return ChalkOutcome::Proven(());
        }

        let mut ambiguous = false;
        for goal in goals {
            let canonical_goal = goal.into_closed_goal(INTER);
            crate::profile::metric::SOLVER_GOALS.inc();
            let started = Instant::now();
            let budget = SolverBudget::new(SOLVER_QUANTUM_BUDGET);
            let solution = self.impl_bounds_solver.solve_limited(
                self.program.database(),
                &canonical_goal,
                &|| budget.should_continue(),
            );
            crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND
                .record("impl_bounds", started.elapsed());
            if budget.exhausted() {
                return ChalkOutcome::Exhausted;
            }
            let Some(solution) = solution else {
                return ChalkOutcome::NoSolution;
            };
            if solution.is_ambig() {
                crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
                ambiguous = true;
            }
        }
        if ambiguous {
            ChalkOutcome::Ambiguous(None)
        } else {
            ChalkOutcome::Proven(())
        }
    }

    fn normalize_assoc_type(
        &mut self,
        trait_goal: &crate::trait_selection::TraitGoal,
        assoc_name: &str,
        selected_impl: Option<&(ImplRef, GenericArgs)>,
        table: &InferenceTable,
    ) -> ChalkOutcome<AssocProjectionResult> {
        let Some(assoc_type_ref) = self
            .program
            .associated_ty_ref(trait_goal.trait_ref(), assoc_name)
        else {
            return ChalkOutcome::NoSolution;
        };
        let binders = GenericBinderEnv::empty();
        let lowerer =
            ChalkLowerer::new(&binders).with_associated_tys(self.program.associated_tys());
        let Some(projection) = lowerer.projection_alias(assoc_type_ref, trait_goal, table) else {
            return ChalkOutcome::Unsupported;
        };

        // A selected impl value and an opaque associated equality are already exact proof terms in
        // the materialized Chalk program. Instantiate that evidence directly before asking the SLG
        // forest to search. Besides being cheaper, this avoids aggregate `Normalize` guidance that
        // cannot choose between a shallow alias and the same alias's normal form.
        let selected_value = selected_impl.and_then(|(impl_ref, args)| {
            let args = lowerer.selected_impl_args(args, table, &projection.variables)?;
            self.program
                .selected_associated_ty_value(*impl_ref, assoc_type_ref, &args)
        });
        if let Some(ty) = self
            .program
            .opaque_associated_ty_value(&projection.alias)
            .or(selected_value)
            .and_then(|projected_ty| {
                raise::infer_ty_from_chalk_projection(
                    &projected_ty,
                    &projection.variables,
                    &ProjectionAnswerVars::empty(),
                )
            })
        {
            return ChalkOutcome::Proven(AssocProjectionResult {
                ty,
                applicability: TraitApplicability::Yes,
                table: table.clone(),
            });
        }

        // Without exact impl or opaque-bound evidence, a goal containing body inference slots is
        // speculative. It can still produce useful equality evidence for a simple generic impl,
        // but it must not enumerate the whole visible impl universe for the same budget as a
        // settled semantic query. Body inference will retry with a more precise goal after its
        // fixed-point pass learns more about those slots.
        let has_live_inference = trait_goal.application.args.iter().any(|arg| arg.has_var())
            || trait_goal
                .associated_types
                .iter()
                .any(|binding| binding.ty.has_var());
        let quantum_budget = if selected_impl.is_none() && has_live_inference {
            OPEN_PROJECTION_QUANTUM_BUDGET
        } else {
            SOLVER_QUANTUM_BUDGET
        };

        // Ask Chalk for the one existential result type in:
        //
        // `Normalize(<Self as Trait>::Assoc -> ?Result)`
        //
        // The binder also includes any ordinary project inference variables used by the receiver
        // goal. If Chalk answers `?Result = ?T`, the decoder maps that bound var back to the same
        // rust-glancer `Ty::InferVar`, then commits only the concrete equalities it can decode.
        let normalize = Normalize {
            alias: projection.alias.clone(),
            ty: projection.variables.result_ty(),
        };
        let chalk_goal = GoalData::Quantified(
            QuantifierKind::Exists,
            Binders::new(
                projection.variables.variable_kinds_with_result(),
                DomainGoal::Normalize(normalize).cast(INTER),
            ),
        )
        .intern(INTER);

        let canonical_goal = chalk_goal.into_peeled_goal(INTER);
        crate::profile::metric::SOLVER_GOALS.inc();
        let started = Instant::now();
        let budget = SolverBudget::new(quantum_budget);
        let solution = self.assoc_projection_solver.solve_limited(
            self.program.database(),
            &canonical_goal,
            &|| budget.should_continue(),
        );
        let elapsed = started.elapsed();
        crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND.record("assoc_projection", elapsed);
        if budget.exhausted() {
            return ChalkOutcome::Exhausted;
        }
        let Some(solution) = solution else {
            return ChalkOutcome::NoSolution;
        };
        if solution.is_ambig() {
            crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
        }

        let applicability = if solution.is_ambig() {
            TraitApplicability::Maybe
        } else {
            TraitApplicability::Yes
        };
        if let Some(subst) = solution.definite_subst(INTER) {
            return match Self::projection_result_from_subst(
                &projection,
                &subst,
                table,
                applicability,
            ) {
                Ok(result) if solution.is_ambig() => ChalkOutcome::Ambiguous(Some(result)),
                Ok(result) => ChalkOutcome::Proven(result),
                Err(ProjectionAnswerFailure::Unsupported) => ChalkOutcome::Unsupported,
                Err(ProjectionAnswerFailure::Conflicting) => ChalkOutcome::NoSolution,
            };
        }

        ChalkOutcome::Ambiguous(None)
    }

    /// Decode one substitution produced for the projection existential and its input variables.
    fn projection_result_from_subst(
        projection: &ProjectionAliasLowering,
        subst: &Canonical<ConstrainedSubst<RgChalkInterner>>,
        table: &InferenceTable,
        applicability: TraitApplicability,
    ) -> Result<AssocProjectionResult, ProjectionAnswerFailure> {
        let subst_args = subst.value.subst.as_slice(INTER);
        let mut table = table.clone();

        let Some(answer_vars) =
            ProjectionAnswerVars::from_subst_args(&projection.variables, subst_args)
        else {
            return Err(ProjectionAnswerFailure::Unsupported);
        };

        for (index, var) in projection.variables.iter_project_vars() {
            let Some(project_arg) = subst_args.get(index) else {
                return Err(ProjectionAnswerFailure::Unsupported);
            };
            let GenericArgData::Ty(project_ty) = project_arg.data(INTER) else {
                return Err(ProjectionAnswerFailure::Unsupported);
            };
            if let Some(evidence) = raise::infer_ty_from_chalk_projection(
                project_ty,
                &projection.variables,
                &answer_vars,
            ) && table
                .try_unify(&crate::Ty::var_for_kind(InferVarKind::Type, var), &evidence)
                .is_err()
            {
                return Err(ProjectionAnswerFailure::Conflicting);
            }
        }

        let Some(projected_arg) = subst_args.get(projection.variables.result_index()) else {
            return Err(ProjectionAnswerFailure::Unsupported);
        };
        let GenericArgData::Ty(projected_ty) = projected_arg.data(INTER) else {
            return Err(ProjectionAnswerFailure::Unsupported);
        };
        let Some(ty) = raise::infer_ty_from_chalk_projection(
            projected_ty,
            &projection.variables,
            &answer_vars,
        ) else {
            return Err(ProjectionAnswerFailure::Unsupported);
        };
        Ok(AssocProjectionResult {
            ty,
            applicability,
            table,
        })
    }
}
