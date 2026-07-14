//! Query-facing Chalk solver state.
//!
//! `TraitSelectionQuery` sends canonical project goals through this module. It makes sure the
//! goal's semantic definitions exist in the shared Chalk program, runs the appropriate solver
//! forest, and translates projection evidence back into rust-glancer inference facts.
//!
//! The state is serialized behind one mutex because a Chalk solver forest mutates as it records
//! answers. Impl-predicate checks and associated-type projection use different forests: the first
//! only needs one answer, while the second must retain the substitution for its result type.

use std::sync::Mutex;
use std::time::Instant;

use chalk_engine::solve::SLGSolver;
use chalk_ir::cast::Cast;
use chalk_ir::{Binders, DomainGoal, GenericArgData, GoalData, Normalize, QuantifierKind};
use chalk_solve::Solver;
use chalk_solve::ext::GoalExt;
use rg_def_map::DefMapSource;
use rg_ir_model::TraitApplicability;
use rg_semantic_ir::{CrateItemQuery, ItemStoreSource};

use super::interner::RgChalkInterner;
use super::lower::{ChalkLowerer, GenericBinderEnv};
use super::program::ChalkProgramState;
use super::projection::ProjectionAnswerVars;
use super::raise;
use crate::inference::{InferVarKind, InferenceSubstitution, InferenceTable};
use crate::trait_selection::{AssocProjectionResult, TraitSelectionCache};
use crate::{Clause, ItemPathQuery};

const INTER: RgChalkInterner = RgChalkInterner;
const SOLVER_MAX_SIZE: usize = 32;

/// Long-lived Chalk state owned by one `TraitSelectionCache`.
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
    /// `None` means the bounded Chalk adapter cannot model the clauses. Body-local closures take
    /// this path because their callable signature lives in Body IR rather than the shared program.
    pub(crate) fn impl_bounds_applicability<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        cache: &TraitSelectionCache,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Result<Option<TraitApplicability>, I::Error>
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
            return Ok(None);
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        state.program.ensure_for_clauses(
            item_paths,
            crate_items,
            cache,
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
        cache: &TraitSelectionCache,
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
            .ensure_for_clauses(item_paths, crate_items, cache, clauses, None)
    }

    /// Normalize one associated type and return any inference evidence carried by Chalk's answer.
    pub(crate) fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        cache: &TraitSelectionCache,
        goal: &crate::trait_selection::TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error>
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
            return Ok(None);
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        state
            .program
            .ensure_for_goal(item_paths, crate_items, cache, goal, table)?;
        Ok(state.normalize_assoc_type(goal, assoc_name, table))
    }
}

impl ChalkSolverState {
    fn new() -> Self {
        Self {
            program: ChalkProgramState::new(),
            // Impl-bound checks only need to know whether a candidate obligation has at least one
            // answer, while associated projection needs the substitution for the projected type.
            // Keep separate SLG forests so the two goal modes do not share different answer limits.
            impl_bounds_solver: SLGSolver::new(SOLVER_MAX_SIZE, Some(1)),
            assoc_projection_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
        }
    }

    fn impl_bounds_applicability(
        &mut self,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Option<TraitApplicability> {
        let binders = GenericBinderEnv::empty();
        let lowerer =
            ChalkLowerer::new(&binders).with_associated_tys(self.program.associated_tys());
        let Some(goals) = lowerer.candidate_where_goals(clauses, subst, table) else {
            return None;
        };
        if goals.is_empty() {
            return Some(TraitApplicability::Yes);
        }

        let mut applicability = TraitApplicability::Yes;
        for goal in goals {
            let canonical_goal = goal.into_closed_goal(INTER);
            crate::profile::metric::SOLVER_GOALS.inc();
            let started = Instant::now();
            let solution = self
                .impl_bounds_solver
                .solve(self.program.database(), &canonical_goal);
            crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND
                .record("impl_bounds", started.elapsed());
            let Some(solution) = solution else {
                return None;
            };
            if solution.is_ambig() {
                crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
                applicability = applicability.and(TraitApplicability::Maybe);
            }
        }
        Some(applicability)
    }

    fn normalize_assoc_type(
        &mut self,
        trait_goal: &crate::trait_selection::TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Option<AssocProjectionResult> {
        let Some(assoc_type_ref) = self
            .program
            .associated_ty_ref(trait_goal.trait_ref(), assoc_name)
        else {
            return None;
        };
        let binders = GenericBinderEnv::empty();
        let lowerer =
            ChalkLowerer::new(&binders).with_associated_tys(self.program.associated_tys());
        let Some(projection) = lowerer.projection_alias(assoc_type_ref, trait_goal, table) else {
            return None;
        };
        // Ask Chalk for the one existential result type in:
        //
        // `Normalize(<Self as Trait>::Assoc -> ?Result)`
        //
        // The binder also includes any ordinary project inference variables used by the receiver
        // goal. If Chalk answers `?Result = ?T`, the decoder maps that bound var back to the same
        // rust-glancer `Ty::InferVar`, then commits only the concrete equalities it can decode.
        let normalize = Normalize {
            alias: projection.alias,
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
        let solution = self
            .assoc_projection_solver
            .solve(self.program.database(), &canonical_goal);
        let elapsed = started.elapsed();
        crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND.record("assoc_projection", elapsed);
        let Some(solution) = solution else {
            return None;
        };
        if solution.is_ambig() {
            crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
        }

        let applicability = if solution.is_ambig() {
            TraitApplicability::Maybe
        } else {
            TraitApplicability::Yes
        };
        let Some(subst) = solution.definite_subst(INTER) else {
            return None;
        };
        let subst_args = subst.value.subst.as_slice(INTER);
        let mut table = table.clone();

        let Some(answer_vars) =
            ProjectionAnswerVars::from_subst_args(&projection.variables, subst_args)
        else {
            return None;
        };

        for (index, var) in projection.variables.iter_project_vars() {
            let Some(project_arg) = subst_args.get(index) else {
                return None;
            };
            let GenericArgData::Ty(project_ty) = project_arg.data(INTER) else {
                return None;
            };
            if let Some(evidence) = raise::infer_ty_from_chalk_projection(
                project_ty,
                &projection.variables,
                &answer_vars,
            ) {
                if table
                    .try_unify(&crate::Ty::var_for_kind(InferVarKind::Type, var), &evidence)
                    .is_err()
                {
                    return None;
                }
            }
        }

        let Some(projected_arg) = subst_args.get(projection.variables.result_index()) else {
            return None;
        };
        let GenericArgData::Ty(projected_ty) = projected_arg.data(INTER) else {
            return None;
        };
        let Some(ty) = raise::infer_ty_from_chalk_projection(
            projected_ty,
            &projection.variables,
            &answer_vars,
        ) else {
            return None;
        };
        Some(AssocProjectionResult {
            ty,
            applicability,
            table,
        })
    }
}
