//! Body-facing type relations, substitutions, and pending obligations.
//!
//! All values in this layer are arena types from the same solver operation. Declaration types
//! enter once through lowering; finalization is the only path back to persistent semantic types.

use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
};

use rg_ir_model::GenericParamRef;
use rg_std::UniqueVec;
use rustc_next_trait_solver::{
    delegate::SolverDelegate,
    solve::{GoalStalledOn, HasChanged, SolverDelegateEvalExt},
};
use rustc_type_ir::{
    self as ir, InferCtxtLike, TypeFoldable, TypeFolder, TypeSuperFoldable, TypeSuperVisitable,
    TypeVisitable, TypeVisitableExt,
    data_structures::HashMap,
    inherent::{GenericArg as _, IntoKind},
    relate::solver_relating::RelateExt,
};

use super::{
    Clause, Const, DefId, GenericArg, GenericArgs, List, Outcome, ParamEnv, Predicate, Region,
    Solver, SolverInterner, Ty,
    infer::{OpaqueEntries, Snapshot},
};

// Safety cap for passes over the goal queue in one fulfillment call. A chain of goals can keep
// changing inference variables without finishing, so progress alone cannot bound this work.
const MAX_OBLIGATION_FULFILLMENT_ROUNDS: usize = 128;

type I<'s> = SolverInterner<'s>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceConflict;

/// A solver question that has not finished yet, such as `?T: Clone` or
/// `<Iter<?T> as Iterator>::Item = ?Item`. Remember what stopped it so an unchanged question
/// does not start the same search at every body-inference checkpoint.
#[derive(Clone)]
pub(super) struct Pending<'s> {
    goal: ir::solve::Goal<I<'s>, Predicate<'s>>,
    stalled: Option<GoalStalledOn<I<'s>>>,
    normalization: Option<(Ty<'s>, Ty<'s>)>,
    unavailable: Option<GoalInputs<'s>>,
}

/// Missing source information cannot improve until the goal's inference inputs change. Keep
/// those inputs after rolling back an unavailable evaluation, just as the compiler records the
/// variables which stalled an ambiguous goal. Numeric fallback is new evidence too: `?int: Trait`
/// may avoid an unsupported impl once the integer becomes a known `u32`.
#[derive(Clone)]
struct GoalInputs<'s> {
    args: UniqueVec<GenericArg<'s>>,
    // Relating two variables or defining an opaque type can help a goal even if its input types
    // still contain variables. Watch those changes as well as concrete type assignments.
    sub_roots: Vec<ir::TyVid>,
    opaques: OpaqueEntries,
}

impl<'s> GoalInputs<'s> {
    fn new(solver: &Solver<'s>, goal: ir::solve::Goal<I<'s>, Predicate<'s>>) -> Self {
        struct Inputs<'a, 's> {
            solver: &'a Solver<'s>,
            args: UniqueVec<GenericArg<'s>>,
        }

        impl<'s> ir::TypeVisitor<I<'s>> for Inputs<'_, 's> {
            type Result = ();
            fn visit_ty(&mut self, ty: Ty<'s>) {
                let ty = self.solver.shallow_resolve(ty);
                if matches!(ty.kind(), ir::Infer(_)) {
                    self.args.push(ty.into());
                } else if ty.has_non_region_infer() {
                    ty.super_visit_with(self);
                }
            }

            fn visit_const(&mut self, c: Const<'s>) {
                let c = self.solver.shallow_resolve_const(c);
                if matches!(c.kind(), ir::ConstKind::Infer(_)) {
                    self.args.push(c.into());
                } else if c.has_non_region_infer() {
                    c.super_visit_with(self);
                }
            }
        }
        let mut inputs = Inputs {
            solver,
            args: UniqueVec::new(),
        };
        goal.visit_with(&mut inputs);
        let sub_roots = inputs
            .args
            .iter()
            .filter_map(|arg| match arg.as_ty()?.kind() {
                ir::Infer(ir::TyVar(vid)) => Some(solver.sub_unification_table_root_var(vid)),
                _ => None,
            })
            .collect();
        Self {
            args: inputs.args,
            sub_roots,
            opaques: solver.opaque_types_storage_num_entries(),
        }
    }

    fn changed(&self, solver: &Solver<'s>) -> bool {
        self.opaques != solver.opaque_types_storage_num_entries()
            || self.args.iter().any(|&arg| solver.is_changed_arg(arg))
            || self
                .sub_roots
                .iter()
                .any(|&root| solver.sub_unification_table_root_var(root) != root)
    }
}

/// A body or standalone type query has one table. Speculative callers can use a snapshot for
/// relations, or fork the table when a candidate must return its complete inference evidence.
///
/// The table keeps three things together: assignments such as `?T = u8`, assumptions from the
/// containing declaration such as `T: Clone`, and questions still waiting for an answer. Types
/// passed around the body hold variable IDs, so learning `?T = u8` also updates every `Vec<?T>`
/// that refers to it. The type nodes themselves do not need to be rewritten.
#[derive(Clone)]
pub struct InferenceTable<'s> {
    solver: Solver<'s>,
    env: ParamEnv<'s>,
    pub(super) pending: RefCell<Vec<Pending<'s>>>,
    failed: Cell<bool>,
    projections: RefCell<Vec<(Ty<'s>, Ty<'s>)>>,
}

impl<'s> InferenceTable<'s> {
    pub fn new(solver: Solver<'s>, env: ParamEnv<'s>) -> Self {
        Self {
            solver,
            env,
            pending: RefCell::default(),
            failed: Cell::new(false),
            projections: RefCell::default(),
        }
    }

    /// Candidate conditions are evaluated with the body's assignments and assumptions, but are
    /// not made responsible for unrelated obligations that the body still needs to finish.
    /// All forks share type storage; each fork has its own assignments and pending goals.
    ///
    /// Callback reporting still goes through the shared interner. Candidate lookup also needs
    /// a `CallbackScope` around preparing and checking that candidate, including declaration
    /// reads which happen before proving its bounds.
    pub fn probe(&self) -> Self {
        self.interner().profile(|p| p.probes += 1);
        let result = Self::new(self.solver.clone(), self.env);
        *result.projections.borrow_mut() = self.projections.borrow().clone();
        result
    }

    /// Prepare an operation in this table, keeping its work only when it returns a usable result.
    /// For example, preparing `make::<T>()` can allocate variables and register bounds before a
    /// missing signature makes us abandon the call. None of that work may reach the next call.
    ///
    /// Assignments use the solver's undo log instead of copying every existing variable. Keep
    /// the caller's pending work aside while the closure runs: it can register or fulfill its
    /// own goals without consuming the caller's goals. On success, append the new work to the
    /// original queues. On `None`, an error, or unavailable callback data, restore the originals.
    pub fn commit_if_some<T, E>(
        &self,
        infer: impl FnOnce(&Self) -> Result<Option<T>, E>,
    ) -> Result<Option<T>, E> {
        let callbacks = self.interner().track_callbacks();
        let transaction = InferenceTransaction {
            table: self,
            snapshot: Some(self.solver.snapshot()),
            pending: self.pending.take(),
            projections: self.projections.take(),
            failed: self.failed.replace(false),
        };
        let result = infer(self)?.filter(|_| callbacks.failure().is_none());
        if result.is_some() {
            transaction.commit();
        }
        Ok(result)
    }

    pub fn fallback_numeric(&self) {
        self.solver.fallback_numeric();
    }

    pub fn has_pending_projection(&self, ty: Ty<'s>) -> bool {
        let ty = self.resolve_root_var(ty);
        ty.is_var()
            && self
                .projections
                .borrow()
                .iter()
                .any(|(destination, _)| self.resolve_root_var(*destination) == ty)
    }

    pub fn interner(&self) -> I<'s> {
        self.solver.interner
    }

    pub fn environment(&self) -> ParamEnv<'s> {
        self.env
    }

    pub fn new_type_var(&self) -> Ty<'s> {
        self.solver.next_ty_infer()
    }

    pub fn new_integer_var(&self) -> Ty<'s> {
        self.solver.next_int_var()
    }

    pub fn new_float_var(&self) -> Ty<'s> {
        self.solver.next_float_var()
    }

    pub fn new_const_var(&self) -> Const<'s> {
        self.solver.next_const_infer()
    }

    pub fn new_region_var(&self) -> Region<'s> {
        self.solver.next_region_infer()
    }

    /// Follow assignments only at the outermost type. `?T = Vec<?U>` resolves to `Vec<?U>`;
    /// resolving the `?U` inside it is a separate traversal.
    pub fn resolve_root_var(&self, ty: impl Borrow<Ty<'s>>) -> Ty<'s> {
        self.solver.shallow_resolve(*ty.borrow())
    }

    /// Expand known assignments for comparing an operation's inputs between retries.
    /// Remaining variables still belong to this table; this does not create the compiler's
    /// independent, numbered canonical query slots.
    pub fn canonicalize(&self, ty: &Ty<'s>) -> Ty<'s> {
        self.resolve(*ty)
    }

    pub fn resolve<T: TypeFoldable<I<'s>>>(&self, value: T) -> T {
        self.solver.resolve_vars_if_possible(value)
    }

    pub fn unify(&self, a: impl Borrow<Ty<'s>>, b: impl Borrow<Ty<'s>>) {
        let _ = self.try_unify(a, b);
    }

    /// Ask that two types be equal, keeping the assignments only if the relation succeeds.
    /// Relating `Vec<?T>` to `Vec<u8>` learns `?T = u8`. A relation involving an associated type
    /// can also add a goal that needs trait solving before the equality is known.
    pub fn try_unify(
        &self,
        a: impl Borrow<Ty<'s>>,
        b: impl Borrow<Ty<'s>>,
    ) -> Result<(), InferenceConflict> {
        let (a, b) = (self.resolve_root_var(a), self.resolve_root_var(b));
        if a.is_unknown() || b.is_unknown() {
            return Ok(());
        }

        // Relating types can read declarations before it produces any trait goals. Check for
        // missing callback data as well as ordinary mismatches. In either case, the snapshot
        // restores this relation's assignments, and its goals never enter the pending queue.
        let callbacks = self.interner().track_callbacks();
        let snapshot = self.solver.snapshot();
        // An unresolved source component supplies no evidence. Give it a throwaway variable for
        // this relation, so a useful sibling can constrain the body without propagating Error.
        let a = self.instantiate_unknowns(a);
        let b = self.instantiate_unknowns(b);
        let goals = self
            .solver
            .relate(self.env, a, ir::Invariant, b, ())
            .map_err(|_| InferenceConflict)?;
        if callbacks.failure().is_some() {
            return Err(InferenceConflict);
        }
        self.pending
            .borrow_mut()
            .extend(goals.into_iter().map(|goal| Pending {
                goal,
                stalled: None,
                normalization: None,
                unavailable: None,
            }));
        snapshot.commit();
        Ok(())
    }

    /// Generic arguments include consts and regions as well as types. Use the compiler's relation
    /// here too, so selecting an array impl retains its inferred length alongside its element.
    /// If a later argument needs unavailable data, also undo assignments for earlier arguments.
    pub fn try_unify_args(
        &self,
        a: GenericArgs<'s>,
        b: GenericArgs<'s>,
    ) -> Result<(), InferenceConflict> {
        let callbacks = self.interner().track_callbacks();
        let snapshot = self.solver.snapshot();
        let goals = self
            .solver
            .relate(self.env, a, ir::Invariant, b, ())
            .map_err(|_| InferenceConflict)?;
        if callbacks.failure().is_some() {
            return Err(InferenceConflict);
        }
        self.pending
            .borrow_mut()
            .extend(goals.into_iter().map(|goal| Pending {
                goal,
                stalled: None,
                normalization: None,
                unavailable: None,
            }));
        snapshot.commit();
        Ok(())
    }

    pub fn register(&self, clause: Clause<'s>) {
        self.pending.borrow_mut().push(Pending {
            goal: ir::solve::Goal::new(self.interner(), self.env, clause),
            stalled: None,
            normalization: None,
            unavailable: None,
        });
    }

    pub fn prove(&self, clauses: impl IntoIterator<Item = Clause<'s>>) -> Outcome {
        for clause in clauses {
            self.register(clause);
        }
        self.fulfill()
    }

    /// Retry stalled goals only after their inputs change. A projection and a bound on its output
    /// stay in this queue together; there is no separate concrete query that loses their variables.
    ///
    /// Queue order matters within one pass. Suppose `?Item: Clone` comes before
    /// `<Iter<u8> as Iterator>::Item = ?Item`. The first goal waits; the second learns `?Item = u8`.
    /// We need another pass to revisit the first goal with that assignment. Evaluating one root
    /// goal does not revisit the other roots in this queue.
    ///
    /// Keep making passes while a goal changes inference state. This also serves standalone type
    /// queries, which have no body retry loop to finish the work for them. Body callers can do
    /// field or method lookup after this returns, add more goals, and call fulfillment again.
    /// If all remaining goals are waiting for more information, return to the caller.
    ///
    /// An ambiguous answer can still supply useful assignments. An answer that relied on missing
    /// source data cannot: roll back that goal's trial before keeping it in the queue.
    pub fn fulfill(&self) -> Outcome {
        let cx = self.interner();
        cx.profile(|p| p.fulfillments += 1);
        if self.env.unavailable {
            return Outcome::Unavailable;
        }
        for _ in 0..MAX_OBLIGATION_FULFILLMENT_ROUNDS {
            if cx.is_cancelled() {
                return Outcome::Unavailable;
            }
            // Work outside the RefCell borrow, retaining unfinished goals in the same buffer.
            // Successful goals have already written their evidence into the inference variables;
            // later rounds and registrations can reuse the queue's allocation.
            let mut pending = std::mem::take(&mut *self.pending.borrow_mut());
            let mut progress = false;
            let mut unavailable = false;
            pending.retain_mut(|pending| {
                cx.profile(|p| p.pending_visits += 1);
                if pending.goal.predicate.references_error()
                    || pending.goal.param_env.references_error()
                {
                    unavailable = true;
                    return true;
                }
                if pending
                    .unavailable
                    .as_ref()
                    .is_some_and(|inputs| !inputs.changed(&self.solver))
                {
                    cx.profile(|p| p.unavailable_reuses += 1);
                    unavailable = true;
                    return true;
                }
                pending.unavailable = None;
                // One queued question is a root goal, such as `Vec<?T>: Clone`. The compiler
                // may try several impls and solve their bounds while answering it. Missing data
                // anywhere in that work makes this root unavailable; the next queued question
                // starts a new scope and can still make progress.
                let callbacks = cx.track_callbacks();
                // The delegate can recognize a still-unknown trait receiver without creating
                // an evaluation context or a rollback snapshot. Keep it pending for later
                // evidence, just as rust-analyzer's fulfillment context does.
                if let Some(certainty) = self.solver.compute_goal_fast_path(pending.goal, ()) {
                    cx.profile(|p| p.fast_path_evaluations += 1);
                    if certainty == ir::solve::Certainty::Yes {
                        if let Some(projection) = pending.normalization {
                            self.projections.borrow_mut().retain(|p| *p != projection);
                        }
                        return false;
                    }
                    pending.stalled = None;
                    return true;
                }
                let snapshot = self.solver.snapshot();
                let was_tainted = self.solver.is_tainted();
                cx.profile(|p| p.evaluations += 1);
                let result =
                    self.solver
                        .evaluate_root_goal(pending.goal, (), pending.stalled.take());
                // The compiler's Result does not include our missing-data reports. Check those
                // before accepting even a successful result and the assignments it produced.
                let unavailable_reason = callbacks.failure();
                if unavailable_reason.is_some() || self.solver.is_tainted() && !was_tainted {
                    cx.profile(|p| *p.outcomes.entry("unavailable").or_default() += 1);
                    tracing::debug!(?unavailable_reason, goal = ?pending.goal, "trait obligation unavailable");
                    // Speculative assignments must not become the inputs which decide whether
                    // to retry. Observe the caller's variables after restoring its snapshot.
                    drop(snapshot);
                    pending.unavailable = Some(GoalInputs::new(&self.solver, pending.goal));
                    unavailable = true;
                    return true;
                }
                match result {
                    Ok(result) => {
                        cx.profile(|p| {
                            *p.outcomes
                                .entry(if result.certainty == ir::solve::Certainty::Yes {
                                    "proven"
                                } else {
                                    "ambiguous"
                                })
                                .or_default() += 1
                        });
                        // Even an ambiguous answer can change variables that earlier goals use.
                        // Proving a goal without changing inference state needs no extra pass.
                        progress |= result.has_changed == HasChanged::Yes;
                        snapshot.commit();
                        if result.certainty == ir::solve::Certainty::Yes {
                            if let Some(projection) = pending.normalization {
                                self.projections.borrow_mut().retain(|p| *p != projection);
                            }
                            false
                        } else {
                            pending.goal = result.goal;
                            pending.stalled = result.stalled_on;
                            true
                        }
                    }
                    Err(_) => {
                        cx.profile(|p| *p.outcomes.entry("no_solution").or_default() += 1);
                        // Earlier successful obligations keep their guidance, but a failed root
                        // cannot leave any of its speculative assignments in the table.
                        self.failed.set(true);
                        false
                    }
                }
            });
            *self.pending.borrow_mut() = pending;
            if self.pending.borrow().is_empty() {
                return if self.failed.get() {
                    Outcome::NoSolution
                } else {
                    Outcome::Proven
                };
            }
            if !progress {
                // Every remaining goal has seen the same evidence it would see on another pass.
                // Leave it queued until the caller supplies more information.
                return if unavailable {
                    Outcome::Unavailable
                } else if self.failed.get() {
                    Outcome::NoSolution
                } else {
                    Outcome::Ambiguous
                };
            }
        }
        // Keep accepted assignments and unfinished goals, but do not claim that fulfillment
        // settled: the round limit stopped it while goals were still making progress.
        Outcome::Unavailable
    }

    /// Replace associated types with variables whose answers can arrive later.
    ///
    /// `Vec<<T as Iterator>::Item>` becomes `Vec<?Item>`, with the equality
    /// `<T as Iterator>::Item = ?Item` queued for solving. The caller can keep using `Vec<?Item>`
    /// while other body expressions help determine `T` or the iterator's item type.
    pub fn normalize(&self, ty: Ty<'s>) -> Ty<'s> {
        struct Normalizer<'a, 's>(&'a InferenceTable<'s>);

        impl<'s> TypeFolder<I<'s>> for Normalizer<'_, 's> {
            fn cx(&self) -> I<'s> {
                self.0.interner()
            }

            fn fold_ty(&mut self, ty: Ty<'s>) -> Ty<'s> {
                // Concrete alias-free subtrees cannot acquire a projection. An inference
                // variable still needs resolving because its assigned type may contain one.
                if !ty.has_aliases() && !ty.has_non_region_infer() {
                    return ty;
                }
                let ty = self.0.resolve_root_var(ty).super_fold_with(self);
                if matches!(
                    ty.kind(),
                    ir::Alias(ir::AliasTy {
                        kind: ir::AliasTyKind::Projection { .. }
                            | ir::AliasTyKind::Inherent { .. }
                            | ir::AliasTyKind::Free { .. },
                        ..
                    })
                ) {
                    let normalized = self.0.new_type_var();
                    let previous = self.0.pending.borrow().len();
                    if self.0.try_unify(normalized, ty).is_err() {
                        // We created ?Item expecting the relation to connect it to this alias.
                        // Without that equality, an empty goal queue could look like successful
                        // normalization even though ?Item has no connection to the source type.
                        return self.cx().unknown();
                    }
                    let mut pending = self.0.pending.borrow_mut();
                    if pending.len() > previous {
                        // Keep an alias spelling only while its equality is pending. Once
                        // `<Iter<?T> as Iterator>::Item = ?T` is proved, the still-open ?T is an
                        // ordinary variable and must not acquire the alias as a recursive fallback.
                        self.0.projections.borrow_mut().push((normalized, ty));
                        pending
                            .last_mut()
                            .expect("normalization added a goal")
                            .normalization = Some((normalized, ty));
                    }
                    normalized
                } else {
                    ty
                }
            }
        }
        ty.fold_with(&mut Normalizer(self))
    }

    /// Preserve the known outer shape while making missing children inferable.
    /// `Vec<Unknown>` becomes `Vec<?T>`, but a completely unknown type stays unknown.
    pub fn instantiate_nested_unknowns(&self, ty: impl Borrow<Ty<'s>>) -> Ty<'s> {
        let ty = *ty.borrow();
        if ty.is_unknown() {
            ty
        } else {
            self.instantiate_unknowns(ty)
        }
    }

    pub fn instantiate_unknowns(&self, ty: Ty<'s>) -> Ty<'s> {
        struct Instantiate<'a, 's>(&'a InferenceTable<'s>);

        impl<'s> TypeFolder<I<'s>> for Instantiate<'_, 's> {
            fn cx(&self) -> I<'s> {
                self.0.interner()
            }

            fn fold_ty(&mut self, ty: Ty<'s>) -> Ty<'s> {
                // Interned flags summarize the whole subtree; most ordinary relations have
                // no unknown components, so there is nothing to replace or re-intern.
                if !ty.references_error() {
                    return ty;
                }
                if ty.is_unknown() {
                    self.0.new_type_var()
                } else {
                    ty.super_fold_with(self)
                }
            }

            fn fold_const(&mut self, c: Const<'s>) -> Const<'s> {
                if !c.references_error() {
                    return c;
                }
                if matches!(c.kind(), ir::ConstKind::Error(_)) {
                    self.0.new_const_var()
                } else {
                    c.super_fold_with(self)
                }
            }
        }
        ty.fold_with(&mut Instantiate(self))
    }

    /// Produce a type that can outlive this inference table.
    /// Follow learned assignments, choose defaults for unresolved numeric variables, and replace
    /// other unanswered variables with `Unknown`. A pending projection can retain its spelling,
    /// such as `T::Item`, when that says more than `Unknown` would.
    pub fn finalize(&self, ty: impl Borrow<Ty<'s>>) -> crate::Ty {
        let ty = *ty.borrow();
        struct Finalize<'a, 's>(&'a InferenceTable<'s>, Vec<Ty<'s>>);

        impl<'s> TypeFolder<I<'s>> for Finalize<'_, 's> {
            fn cx(&self) -> I<'s> {
                self.0.interner()
            }

            fn fold_ty(&mut self, t: Ty<'s>) -> Ty<'s> {
                if !t.has_non_region_infer() {
                    return t;
                }
                match self.0.resolve_root_var(t).kind() {
                    ir::Infer(ir::IntVar(_)) => Ty::new(self.cx(), ir::Int(ir::IntTy::I32)),
                    ir::Infer(ir::FloatVar(_)) => Ty::new(self.cx(), ir::Float(ir::FloatTy::F64)),
                    ir::Infer(_) => {
                        let root = self.0.resolve_root_var(t);
                        if self.1.contains(&root) {
                            return self.cx().unknown();
                        }
                        let alias = self
                            .0
                            .projections
                            .borrow()
                            .iter()
                            .find(|(destination, _)| self.0.resolve_root_var(*destination) == root)
                            .map(|(_, alias)| *alias);
                        match alias {
                            Some(alias) => {
                                self.1.push(root);
                                let result = alias.super_fold_with(self);
                                self.1.pop();
                                result
                            }
                            None => self.cx().unknown(),
                        }
                    }
                    _ => self.0.resolve_root_var(t).super_fold_with(self),
                }
            }

            fn fold_const(&mut self, c: Const<'s>) -> Const<'s> {
                if !c.has_non_region_infer() {
                    return c;
                }
                self.0.solver.shallow_resolve_const(c).super_fold_with(self)
            }
        }
        self.interner()
            .raise_ty(ty.fold_with(&mut Finalize(self, Vec::new())))
            .unwrap_or(crate::Ty::Unknown)
    }

    pub fn finalize_args(&self, args: GenericArgs<'s>) -> crate::GenericArgs {
        args.iter()
            .map(|arg| match arg.as_ty() {
                Some(ty) => crate::GenericArg::Type(Box::new(self.finalize(ty))),
                None => self
                    .interner()
                    .raise_args(List::new(self.interner(), &[self.resolve(arg)]))[0]
                    .clone(),
            })
            .collect()
    }

    pub fn params(&self, owner: DefId) -> &'s [GenericParamRef] {
        self.interner().params(owner)
    }

    pub fn lower(&self, ty: &crate::Ty, owner: DefId) -> Ty<'s> {
        self.interner().lower_ty(ty, self.interner().params(owner))
    }
}

/// Hold the caller's goal queues while an operation works on the same inference variables.
/// The guard restores them on every exit, including unwinding; the solver snapshot separately
/// undoes assignments and fresh variables unless committed.
struct InferenceTransaction<'a, 's> {
    table: &'a InferenceTable<'s>,
    snapshot: Option<Snapshot<'a, 's>>,
    pending: Vec<Pending<'s>>,
    projections: Vec<(Ty<'s>, Ty<'s>)>,
    failed: bool,
}

impl InferenceTransaction<'_, '_> {
    fn commit(mut self) {
        self.snapshot.take().expect("active transaction").commit();
    }
}

impl Drop for InferenceTransaction<'_, '_> {
    fn drop(&mut self) {
        if self.snapshot.is_none() {
            // Reuse the original buffers: moving all earlier goals into a fresh buffer on every
            // call would just replace variable-table copying with goal-queue copying.
            self.pending.append(&mut self.table.pending.borrow_mut());
            self.projections
                .append(&mut self.table.projections.borrow_mut());
            self.failed |= self.table.failed.get();
        }
        *self.table.pending.borrow_mut() = std::mem::take(&mut self.pending);
        *self.table.projections.borrow_mut() = std::mem::take(&mut self.projections);
        self.table.failed.set(self.failed);
    }
}

/// Source parameter identities select bindings; spelling and declaration-local indices do not.
/// For `impl<T> Wrapper<T> { fn map<U>(...) }`, the map can hold both `T = u8` and `U = ?U`.
/// Applying it to a signature replaces those declaration parameters with the same live arguments.
#[derive(Debug, Clone, Default)]
pub struct InferenceSubstitution<'s> {
    pub(super) args: HashMap<GenericParamRef, GenericArg<'s>>,
}

impl<'s> InferenceSubstitution<'s> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, param: GenericParamRef, arg: GenericArg<'s>) {
        self.args.insert(param, arg);
    }

    pub fn get(&self, param: GenericParamRef) -> Option<GenericArg<'s>> {
        self.args.get(&param).copied()
    }

    pub(crate) fn identity(cx: I<'s>, params: impl IntoIterator<Item = GenericParamRef>) -> Self {
        let mut subst = Self::new();
        for (index, param) in params.into_iter().enumerate() {
            subst.insert(param, cx.param_arg(param, index));
        }
        subst
    }

    pub fn fresh_for(
        &mut self,
        table: &InferenceTable<'s>,
        params: impl IntoIterator<Item = GenericParamRef>,
    ) {
        for param in params {
            self.args.entry(param).or_insert_with(|| match param {
                GenericParamRef::Type(_) => table.new_type_var().into(),
                GenericParamRef::Lifetime(_) => table.new_region_var().into(),
                GenericParamRef::Const(_) => table.new_const_var().into(),
            });
        }
    }

    pub fn args_for(
        &self,
        cx: I<'s>,
        params: impl IntoIterator<Item = GenericParamRef>,
    ) -> GenericArgs<'s> {
        List::new(
            cx,
            &params
                .into_iter()
                .map(|p| self.get(p).unwrap_or_else(|| cx.unknown_arg(p)))
                .collect::<Vec<_>>(),
        )
    }

    pub fn apply<T: TypeFoldable<I<'s>>>(&self, cx: I<'s>, value: T) -> T {
        struct Apply<'a, 's> {
            cx: I<'s>,
            subst: &'a InferenceSubstitution<'s>,
        }

        impl<'s> TypeFolder<I<'s>> for Apply<'_, 's> {
            fn cx(&self) -> I<'s> {
                self.cx
            }

            fn fold_ty(&mut self, t: Ty<'s>) -> Ty<'s> {
                if !t.has_param() {
                    return t;
                }
                match t.kind() {
                    ir::Param(p) => self.subst.get(p.source).map_or(t, |a| a.expect_ty()),
                    _ => t.super_fold_with(self),
                }
            }

            fn fold_const(&mut self, c: Const<'s>) -> Const<'s> {
                if !c.has_param() {
                    return c;
                }
                match c.kind() {
                    ir::ConstKind::Param(p) => {
                        self.subst.get(p.source).map_or(c, |a| a.expect_const())
                    }
                    _ => c.super_fold_with(self),
                }
            }

            fn fold_region(&mut self, r: Region<'s>) -> Region<'s> {
                match r.kind() {
                    ir::ReEarlyParam(p) => {
                        self.subst.get(p.source).map_or(r, |a| a.expect_region())
                    }
                    _ => r,
                }
            }
        }
        value.fold_with(&mut Apply { cx, subst: self })
    }
}

impl Ty<'_> {
    pub fn is_unknown(self) -> bool {
        matches!(self.kind(), ir::Error(_))
    }

    pub fn has_unknown(self) -> bool {
        self.references_error()
    }

    pub fn has_var(self) -> bool {
        self.has_non_region_infer()
    }

    pub fn is_var(self) -> bool {
        matches!(self.kind(), ir::Infer(_))
    }

    pub fn is_never(self) -> bool {
        matches!(self.kind(), ir::Never)
    }
}
