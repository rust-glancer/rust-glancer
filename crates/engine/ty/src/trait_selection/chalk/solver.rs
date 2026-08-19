//! Query-facing Chalk solver state.
//!
//! `TraitSelectionQuery` sends canonical project goals through this module. It makes sure the
//! goal's semantic definitions exist in the shared Chalk program, runs the appropriate solver
//! forest, and translates projection evidence back into rust-glancer inference facts.
//!
//! The state is serialized behind one mutex because a Chalk solver forest mutates as it records
//! answers. Impl-predicate checks and associated-type projection use different forests: the first
//! only needs one answer, while the second must retain the substitution for its result type.
//!
//! Native selection may also pass one exact impl into associated-type projection. Chalk uses that
//! evidence to prefer the selected impl's value while retaining the solver path for defaults,
//! opaque bounds, and other cases that need the complete program.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreSource};

use super::super::matcher::TraitSelfHead;
use super::evidence::{ProjectionAliasLowering, SolverAnswerVars, SolverVariableEnv};
use super::interner::RgChalkInterner;
use super::lower::{ChalkLowerer, GenericBinderEnv};
use super::program::ChalkProgramState;
use super::raise;
use crate::inference::{InferVarKind, InferenceSubstitution, InferenceTable};
use crate::trait_selection::{AssocProjectionResult, TraitSelectionSession};
use crate::{Clause, GenericArg, GenericArgs, ItemPathQuery, TraitApplication};

const INTER: RgChalkInterner = RgChalkInterner;
const SOLVER_MAX_SIZE: usize = 32;
const SETTLED_GOAL_QUANTUM_BUDGET: usize = 4_096;
const SPECULATIVE_GOAL_QUANTUM_BUDGET: usize = 256;
const UNRESOLVED_PROJECTION_GOAL_QUANTUM_BUDGET: usize = 256;
// Program construction is not resumable like solver search. Avoid admitting a live inference goal
// with a large receiver-compatible root set while it is likely to become more precise later.
const SPECULATIVE_ROOT_IMPL_BUDGET: usize = 64;
// A body may ask thousands of small solver questions. Log only a goal that is independently
// expensive enough to explain a noticeable part of indexing time.
const SLOW_SOLVER_GOAL: Duration = Duration::from_millis(100);

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

/// Why an otherwise valid Chalk substitution cannot become project inference evidence.
enum AnswerFailure {
    /// The answer uses a type shape outside rust-glancer's semantic model.
    Unsupported,
    /// Applying the answer would contradict evidence already present in the caller's table.
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

/// Long-lived Chalk state shared by `TraitSelectionSession`s for one crate view.
///
/// The semantic program grows as new goals mention new traits. Only forests whose goals contain no
/// body-owned inference variables or closure identities stay here. Answers that depend on those
/// identities instead live in `ChalkInferenceCache` and disappear with the body that owns them.
pub(crate) struct ChalkTraitSolver {
    state: Mutex<ChalkSolverState>,
}

/// Solver forests whose answers are valid only within one inference scope.
///
/// The semantic program is still crate-scoped and shared through `ChalkTraitSolver`. A body pass
/// receives a fresh cache so its fixed-point rounds can reuse answers involving that body's
/// closures and inference slots, then drops the forests when the body finishes.
pub(crate) struct ChalkInferenceCache {
    forests: Mutex<ChalkSolverForests>,
    declined_proofs: Mutex<HashMap<Vec<Clause>, DeclinedProof>>,
}

/// A bounded adapter result that cannot become more precise without different input clauses.
#[derive(Clone, Copy)]
enum DeclinedProof {
    Unsupported,
    Exhausted,
}

/// Mutable crate program together with answers safe to reuse across bodies.
struct ChalkSolverState {
    program: ChalkProgramState,
    stable_forests: ChalkSolverForests,
}

/// Separate SLG forests for predicate proof and associated-type projection.
///
/// Both operations submit Chalk goals, but projection adds an explicit result variable while
/// predicate proof maps only the input variables. Keeping the forests separate avoids mixing
/// those goal and answer layouts and lets each retain work for its own query kind.
struct ChalkSolverForests {
    impl_bounds_solver: SLGSolver<RgChalkInterner>,
    assoc_projection_solver: SLGSolver<RgChalkInterner>,
}

impl ChalkInferenceCache {
    pub(crate) fn new() -> Self {
        Self {
            forests: Mutex::new(ChalkSolverForests::new()),
            declined_proofs: Mutex::new(HashMap::new()),
        }
    }

    /// Reuse only declines from this body's exact canonical clauses.
    ///
    /// Solver forests may resume incomplete search, but body inference should not spend another
    /// full allowance on an unchanged obligation merely because an unrelated expression made the
    /// outer fixed point run again. A changed inference solution produces different canonical
    /// clauses and therefore gets a fresh attempt.
    fn declined_proof(&self, clauses: &[Clause]) -> Option<DeclinedProof> {
        self.declined_proofs
            .lock()
            .expect("Chalk declined-proof cache lock should not be poisoned")
            .get(clauses)
            .copied()
    }

    fn remember_declined_proof(&self, clauses: &[Clause], declined: DeclinedProof) {
        self.declined_proofs
            .lock()
            .expect("Chalk declined-proof cache lock should not be poisoned")
            .insert(clauses.to_vec(), declined);
    }
}

impl ChalkTraitSolver {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(ChalkSolverState::new()),
        }
    }

    /// Prove a related set of predicates and return the inference evidence from one Chalk answer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prove_clauses<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        inference_cache: &ChalkInferenceCache,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<InferenceTable>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if let Some(declined) = inference_cache.declined_proof(clauses) {
            crate::profile::metric::DECLINED_GOAL_REUSES.inc();
            return Ok(match declined {
                DeclinedProof::Unsupported => ChalkOutcome::Unsupported,
                DeclinedProof::Exhausted => ChalkOutcome::Exhausted,
            });
        }

        // Trait selection is allowed to refine arguments after `Self` has a selectable head; it
        // must not infer an unconstrained `Self` by enumerating every visible impl. Apart from
        // being an unbounded search in a large crate, choosing from today's impl set would be the
        // wrong Rust inference rule: another impl can be added without changing this call site.
        // Keep the obligation pending before even materializing that trait's impl universe.
        let has_open_self = clauses.iter().any(|clause| {
            let self_ty = match clause {
                Clause::Implemented(application) => application.self_ty(),
                Clause::AliasEq { alias, .. } => {
                    alias.args.first().and_then(crate::GenericArg::as_ty)
                }
            };
            matches!(
                self_ty,
                Some(crate::Ty::InferVar { .. } | crate::Ty::Unknown)
            )
        });
        if has_open_self {
            return Ok(ChalkOutcome::Ambiguous(None));
        }

        // `Ty::Unknown` has no Chalk representation. Validate that adapter boundary before
        // materializing the roots: lowering the goal after a full program build would return the
        // same `Unsupported` outcome.
        let has_unknown = clauses.iter().any(|clause| match clause {
            Clause::Implemented(application) => {
                application.args.iter().any(GenericArg::has_unknown)
            }
            Clause::AliasEq { alias, ty } => {
                alias.args.iter().any(GenericArg::has_unknown) || ty.has_unknown()
            }
        });
        if has_unknown {
            inference_cache.remember_declined_proof(clauses, DeclinedProof::Unsupported);
            return Ok(ChalkOutcome::Unsupported);
        }

        // The ordinary speculative quantum budget starts after program preparation. Avoid
        // admitting a goal whose receiver can actually reach a large root set, but do not reject a
        // precise receiver merely because the same trait has many unrelated impls. The caller
        // retains an exhausted candidate as `Maybe` and can retry after inference becomes concrete.
        let has_live_inference = clauses.iter().any(|clause| match clause {
            Clause::Implemented(application) => application.args.iter().any(GenericArg::has_var),
            Clause::AliasEq { alias, ty } => {
                alias.args.iter().any(GenericArg::has_var) || ty.has_var()
            }
        });
        if has_live_inference {
            let mut root_impl_count = 0usize;
            for clause in clauses {
                let Clause::Implemented(application) = clause else {
                    continue;
                };
                root_impl_count =
                    root_impl_count.saturating_add(Self::speculative_root_impl_count(
                        item_paths,
                        lookup_index,
                        session,
                        application,
                        table,
                    )?);
                if root_impl_count > SPECULATIVE_ROOT_IMPL_BUDGET {
                    inference_cache.remember_declined_proof(clauses, DeclinedProof::Exhausted);
                    return Ok(ChalkOutcome::Exhausted);
                }
            }
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        let program_started = Instant::now();
        let supported = state.program.ensure_for_clauses(
            item_paths,
            crate_items,
            lookup_index,
            session,
            clauses,
            Some(table),
        )?;
        let program_elapsed = program_started.elapsed();
        if program_elapsed >= SLOW_SOLVER_GOAL {
            tracing::debug!(
                elapsed_ms = program_elapsed.as_millis(),
                clause_count = clauses.len(),
                "slow Chalk proof program preparation"
            );
        }
        if !supported {
            inference_cache.remember_declined_proof(clauses, DeclinedProof::Unsupported);
            return Ok(ChalkOutcome::Unsupported);
        }
        let outcome = state.prove_clauses(clauses, table, inference_cache);
        if program_elapsed >= SLOW_SOLVER_GOAL {
            let solver_outcome = match &outcome {
                ChalkOutcome::Proven(_) => "proven",
                ChalkOutcome::Ambiguous(_) => "ambiguous",
                ChalkOutcome::NoSolution => "no_solution",
                ChalkOutcome::Unsupported => "unsupported",
                ChalkOutcome::Exhausted => "exhausted",
            };
            tracing::debug!(solver_outcome, "slow prepared Chalk proof finished");
        }
        match &outcome {
            ChalkOutcome::Unsupported => {
                inference_cache.remember_declined_proof(clauses, DeclinedProof::Unsupported);
            }
            ChalkOutcome::Exhausted => {
                inference_cache.remember_declined_proof(clauses, DeclinedProof::Exhausted);
            }
            ChalkOutcome::Proven(_) | ChalkOutcome::Ambiguous(_) | ChalkOutcome::NoSolution => {}
        }
        Ok(outcome)
    }

    /// Normalize one associated type through Chalk, optionally guided by an exact selected impl.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        inference_cache: &ChalkInferenceCache,
        goal: &crate::trait_selection::TraitGoal,
        associated_ty: rg_ir_model::TypeAliasRef,
        selected_impl: Option<(ImplRef, &InferenceSubstitution)>,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<AssocProjectionResult>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // Projection must not infer an unconstrained `Self` by enumerating every visible impl.
        // Native selection has no exact evidence in this case, and another impl can be added
        // without changing the call site, so keep the result pending before building the trait's
        // transitive Chalk program. An explicitly selected impl remains valid evidence even when
        // its instantiated goal still contains a caller-owned variable.
        if selected_impl.is_none()
            && matches!(
                table.resolve_root_var(goal.self_ty()),
                crate::Ty::InferVar { .. } | crate::Ty::Unknown
            )
        {
            return Ok(ChalkOutcome::Ambiguous(None));
        }
        let has_live_inference = goal.application.args.iter().any(GenericArg::has_var)
            || goal
                .associated_types
                .iter()
                .any(|binding| binding.ty.has_var());
        if selected_impl.is_none()
            && has_live_inference
            && Self::speculative_root_impl_count(
                item_paths,
                lookup_index,
                session,
                &goal.application,
                table,
            )? > SPECULATIVE_ROOT_IMPL_BUDGET
        {
            return Ok(ChalkOutcome::Exhausted);
        }

        let mut state = self
            .state
            .lock()
            .expect("Chalk solver-state lock should not be poisoned");
        let program_started = Instant::now();
        let supported = state.program.ensure_for_goal(
            item_paths,
            crate_items,
            lookup_index,
            session,
            goal,
            associated_ty,
            table,
        )?;
        let program_elapsed = program_started.elapsed();
        if program_elapsed >= SLOW_SOLVER_GOAL {
            tracing::debug!(
                elapsed_ms = program_elapsed.as_millis(),
                selected_impl = selected_impl.is_some(),
                trait_ref = ?goal.trait_ref(),
                ?associated_ty,
                "slow Chalk projection program preparation"
            );
        }
        if !supported {
            return Ok(ChalkOutcome::Unsupported);
        }
        let selected_impl = if let Some((impl_ref, subst)) = selected_impl {
            let generics = item_paths
                .generics()
                .generics(GenericDefRef::Impl(impl_ref))?;
            Some((impl_ref, subst.as_substitution().args_for(&generics)))
        } else {
            None
        };
        Ok(state.normalize_assoc_type(
            goal,
            associated_ty,
            selected_impl.as_ref(),
            table,
            inference_cache,
        ))
    }

    /// Count only impls whose canonical `Self` head can participate in a speculative root goal.
    ///
    /// Chalk still receives the complete trait program once work starts. This count is only the
    /// admission budget, so unrelated receiver types must not make an otherwise small query look
    /// expensive. Blanket and headless impls remain candidates for every receiver.
    fn speculative_root_impl_count<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        application: &TraitApplication,
        table: &InferenceTable,
    ) -> Result<usize, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let Some(visible_impls) = lookup_index.trait_impls_for_trait(application.def) else {
            return Ok(0);
        };
        let Some(self_head) = application
            .self_ty()
            .map(|self_ty| table.resolve_root_var(self_ty))
            .as_ref()
            .and_then(TraitSelfHead::from_ty)
        else {
            return Ok(visible_impls.len());
        };
        Ok(session
            .indexed_trait_impl_candidates(item_paths, application.def, visible_impls, self_head)?
            .len())
    }
}

impl ChalkSolverForests {
    fn new() -> Self {
        Self {
            // Predicate proof and associated projection use separate forests because their goal
            // shapes and answer decoding differ. `expected_answers` is a diagnostic assertion in
            // Chalk, not a work limit; rust-glancer bounds work through `solve_limited` instead.
            impl_bounds_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
            assoc_projection_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
        }
    }
}

impl ChalkSolverState {
    fn new() -> Self {
        Self {
            program: ChalkProgramState::new(),
            stable_forests: ChalkSolverForests::new(),
        }
    }

    fn prove_clauses(
        &mut self,
        clauses: &[Clause],
        table: &InferenceTable,
        inference_cache: &ChalkInferenceCache,
    ) -> ChalkOutcome<InferenceTable> {
        let binders = GenericBinderEnv::empty();
        let lowerer = ChalkLowerer::new(&binders)
            .with_associated_tys(self.program.associated_tys())
            .with_functions(self.program.functions());
        let Some(lowering) = lowerer.predicate_goal(clauses, table) else {
            return ChalkOutcome::Unsupported;
        };
        let canonical_goal = lowering.goal.into_peeled_goal(INTER);
        crate::profile::metric::SOLVER_GOALS.inc();
        let started = Instant::now();

        // A goal that still contains body inference slots is only a speculative question from one
        // fixed-point round. Giving it the settled-query allowance is particularly costly for
        // blanket bounds such as `?T: Debug`: Chalk may explore the entire visible impl universe,
        // only for body inference to ask again after `?T` becomes more precise. Bound that probe
        // like an open projection and preserve the larger allowance for fully known types.
        let has_live_inference = clauses.iter().any(|clause| match clause {
            Clause::Implemented(application) => application.args.iter().any(|arg| arg.has_var()),
            Clause::AliasEq { alias, ty } => {
                alias.args.iter().any(|arg| arg.has_var()) || ty.has_var()
            }
        });
        let has_unresolved_projection = clauses.iter().any(|clause| match clause {
            Clause::Implemented(application) => {
                application.args.iter().any(|arg| arg.has_projection())
            }
            Clause::AliasEq { .. } => true,
        });
        let quantum_budget = if has_live_inference {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("impl_bounds.live_inference");
            SPECULATIVE_GOAL_QUANTUM_BUDGET
        } else if has_unresolved_projection {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("impl_bounds.unresolved_projection");
            UNRESOLVED_PROJECTION_GOAL_QUANTUM_BUDGET
        } else {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("impl_bounds.settled");
            SETTLED_GOAL_QUANTUM_BUDGET
        };
        let budget = SolverBudget::new(quantum_budget);

        // A crate-wide forest is valuable for declaration-owned goals, but body inference slots
        // and closure identities make each answer local to one inference scope. Retaining them in
        // the stable forest makes later existential queries scan results they cannot reuse.
        let cache_stable = clauses.iter().all(|clause| match clause {
            Clause::Implemented(application) => application
                .args
                .iter()
                .all(|arg| !arg.has_var() && !arg.has_closure()),
            Clause::AliasEq { alias, ty } => {
                alias
                    .args
                    .iter()
                    .all(|arg| !arg.has_var() && !arg.has_closure())
                    && !ty.has_var()
                    && !ty.has_closure()
            }
        });
        let solution = if cache_stable {
            self.stable_forests.impl_bounds_solver.solve_limited(
                self.program.database(),
                &canonical_goal,
                &|| budget.should_continue(),
            )
        } else {
            inference_cache
                .forests
                .lock()
                .expect("Chalk inference-cache lock should not be poisoned")
                .impl_bounds_solver
                .solve_limited(self.program.database(), &canonical_goal, &|| {
                    budget.should_continue()
                })
        };
        let elapsed = started.elapsed();
        crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND.record("impl_bounds", elapsed);
        if elapsed >= SLOW_SOLVER_GOAL {
            let solver_answer = if budget.exhausted() {
                "exhausted"
            } else {
                match &solution {
                    Some(solution) if solution.is_ambig() => "ambiguous",
                    Some(_) => "definite",
                    None => "no_solution",
                }
            };
            tracing::debug!(
                goal_kind = "impl_bounds",
                elapsed_ms = elapsed.as_millis(),
                solver_answer,
                clause_count = clauses.len(),
                quantum_budget,
                has_live_inference,
                has_unresolved_projection,
                cache_scope = if cache_stable { "crate" } else { "body" },
                "slow Chalk solver goal"
            );
        }
        let outcome = 'outcome: {
            if budget.exhausted() {
                break 'outcome ChalkOutcome::Exhausted;
            }
            let Some(solution) = solution else {
                break 'outcome ChalkOutcome::NoSolution;
            };
            if solution.is_ambig() {
                crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
            }

            let Some(subst) = solution.definite_subst(INTER) else {
                break 'outcome if solution.is_ambig() {
                    ChalkOutcome::Ambiguous(None)
                } else {
                    ChalkOutcome::Proven(table.clone())
                };
            };
            break 'outcome match Self::table_from_subst(&lowering.variables, &subst, table) {
                Ok(table) if solution.is_ambig() => ChalkOutcome::Ambiguous(Some(table)),
                Ok(table) => ChalkOutcome::Proven(table),
                Err(AnswerFailure::Unsupported) => ChalkOutcome::Unsupported,
                Err(AnswerFailure::Conflicting) => ChalkOutcome::NoSolution,
            };
        };
        match &outcome {
            ChalkOutcome::Proven(_) => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("impl_bounds.proven");
            }
            ChalkOutcome::Ambiguous(_) => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("impl_bounds.ambiguous");
            }
            ChalkOutcome::NoSolution => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("impl_bounds.no_solution");
            }
            ChalkOutcome::Unsupported => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("impl_bounds.unsupported");
            }
            ChalkOutcome::Exhausted => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("impl_bounds.exhausted");
            }
        }
        outcome
    }

    fn normalize_assoc_type(
        &mut self,
        trait_goal: &crate::trait_selection::TraitGoal,
        assoc_type_ref: rg_ir_model::TypeAliasRef,
        selected_impl: Option<&(ImplRef, GenericArgs)>,
        table: &InferenceTable,
        inference_cache: &ChalkInferenceCache,
    ) -> ChalkOutcome<AssocProjectionResult> {
        if !self.program.associated_tys().contains_key(&assoc_type_ref) {
            return ChalkOutcome::NoSolution;
        }
        let binders = GenericBinderEnv::empty();
        let lowerer = ChalkLowerer::new(&binders)
            .with_associated_tys(self.program.associated_tys())
            .with_functions(self.program.functions());
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
                    &SolverAnswerVars::empty(),
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
            SPECULATIVE_GOAL_QUANTUM_BUDGET
        } else {
            SETTLED_GOAL_QUANTUM_BUDGET
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
        if selected_impl.is_some() {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("assoc_projection.selected_impl");
        } else if has_live_inference {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("assoc_projection.live_inference");
        } else {
            crate::profile::metric::SOLVER_GOAL_SHAPES.inc("assoc_projection.settled");
        }
        let started = Instant::now();
        let budget = SolverBudget::new(quantum_budget);
        // Projection answers containing body-owned identities have the same lifetime as the
        // caller's inference table. Keep them in the inference-scoped cache, not the stable
        // crate-wide forest.
        let cache_stable = trait_goal.is_cache_stable();
        let solution = if cache_stable {
            self.stable_forests.assoc_projection_solver.solve_limited(
                self.program.database(),
                &canonical_goal,
                &|| budget.should_continue(),
            )
        } else {
            inference_cache
                .forests
                .lock()
                .expect("Chalk inference-cache lock should not be poisoned")
                .assoc_projection_solver
                .solve_limited(self.program.database(), &canonical_goal, &|| {
                    budget.should_continue()
                })
        };
        let elapsed = started.elapsed();
        crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND.record("assoc_projection", elapsed);
        if elapsed >= SLOW_SOLVER_GOAL {
            let solver_answer = if budget.exhausted() {
                "exhausted"
            } else {
                match &solution {
                    Some(solution) if solution.is_ambig() => "ambiguous",
                    Some(_) => "definite",
                    None => "no_solution",
                }
            };
            tracing::debug!(
                goal_kind = "assoc_projection",
                elapsed_ms = elapsed.as_millis(),
                solver_answer,
                quantum_budget,
                has_live_inference,
                selected_impl = selected_impl.is_some(),
                cache_scope = if cache_stable { "crate" } else { "body" },
                "slow Chalk solver goal"
            );
        }
        let outcome = 'outcome: {
            if budget.exhausted() {
                break 'outcome ChalkOutcome::Exhausted;
            }
            let Some(solution) = solution else {
                break 'outcome ChalkOutcome::NoSolution;
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
                break 'outcome match Self::projection_result_from_subst(
                    &projection,
                    &subst,
                    table,
                    applicability,
                ) {
                    Ok(result) if solution.is_ambig() => ChalkOutcome::Ambiguous(Some(result)),
                    Ok(result) => ChalkOutcome::Proven(result),
                    Err(AnswerFailure::Unsupported) => ChalkOutcome::Unsupported,
                    Err(AnswerFailure::Conflicting) => ChalkOutcome::NoSolution,
                };
            }

            break 'outcome ChalkOutcome::Ambiguous(None);
        };
        match &outcome {
            ChalkOutcome::Proven(_) => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("assoc_projection.proven");
            }
            ChalkOutcome::Ambiguous(_) => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("assoc_projection.ambiguous");
            }
            ChalkOutcome::NoSolution => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("assoc_projection.no_solution");
            }
            ChalkOutcome::Unsupported => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("assoc_projection.unsupported");
            }
            ChalkOutcome::Exhausted => {
                crate::profile::metric::SOLVER_GOAL_OUTCOMES.inc("assoc_projection.exhausted");
            }
        }
        outcome
    }

    /// Decode one substitution produced for the projection existential and its input variables.
    fn projection_result_from_subst(
        projection: &ProjectionAliasLowering,
        subst: &Canonical<ConstrainedSubst<RgChalkInterner>>,
        table: &InferenceTable,
        applicability: TraitApplicability,
    ) -> Result<AssocProjectionResult, AnswerFailure> {
        let subst_args = subst.value.subst.as_slice(INTER);
        let Some(answer_vars) =
            SolverAnswerVars::from_subst_args(&projection.variables, subst_args)
        else {
            return Err(AnswerFailure::Unsupported);
        };
        let table = Self::table_from_subst(&projection.variables, subst, table)?;

        let Some(projected_arg) = subst_args.get(projection.variables.result_index()) else {
            return Err(AnswerFailure::Unsupported);
        };
        let GenericArgData::Ty(projected_ty) = projected_arg.data(INTER) else {
            return Err(AnswerFailure::Unsupported);
        };
        let Some(ty) = raise::infer_ty_from_chalk_projection(
            projected_ty,
            &projection.variables,
            &answer_vars,
        ) else {
            return Err(AnswerFailure::Unsupported);
        };
        Ok(AssocProjectionResult {
            ty,
            applicability,
            table,
        })
    }

    /// Apply equality evidence for project variables shared by proof and projection queries.
    fn table_from_subst(
        variables: &SolverVariableEnv,
        subst: &Canonical<ConstrainedSubst<RgChalkInterner>>,
        table: &InferenceTable,
    ) -> Result<InferenceTable, AnswerFailure> {
        let subst_args = subst.value.subst.as_slice(INTER);
        let Some(answer_vars) = SolverAnswerVars::from_subst_args(variables, subst_args) else {
            return Err(AnswerFailure::Unsupported);
        };
        let mut table = table.clone();
        for (index, var) in variables.iter_vars() {
            let Some(project_arg) = subst_args.get(index) else {
                return Err(AnswerFailure::Unsupported);
            };
            let GenericArgData::Ty(project_ty) = project_arg.data(INTER) else {
                return Err(AnswerFailure::Unsupported);
            };
            if let Some(evidence) =
                raise::infer_ty_from_chalk_projection(project_ty, variables, &answer_vars)
                && table
                    .try_unify(&crate::Ty::var_for_kind(InferVarKind::Type, var), &evidence)
                    .is_err()
            {
                return Err(AnswerFailure::Conflicting);
            }
        }
        Ok(table)
    }
}
