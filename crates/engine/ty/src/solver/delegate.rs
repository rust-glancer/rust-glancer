//! Connecting the compiler's goal evaluation to our inference context.
//!
//! The compiler solver replaces caller-specific variables with numbered slots before exploring
//! a goal. These are its canonical queries: two callers with different variable ids can ask the
//! same question. The delegate builds a fresh context for those slots and supplies the remaining
//! inference callbacks expected by the compiler algorithms.

use std::ops::Deref;

use ir::solve::{Certainty, FetchEligibleAssocItemResponse, Goal, NoSolution, VisibleForLeakCheck};
use rustc_next_trait_solver::delegate::SolverDelegate;
use rustc_type_ir::{
    self as ir, InferCtxtLike, Interner as _, TypeFoldable, TypeFolder, TypeSuperFoldable,
    inherent::{Const as _, GenericArg as _, IntoKind, Region as _, Ty as _},
};

use super::{
    Const, DeclarationKind, DefId, GenericArg, GenericArgs, InferCtxt, ParamEnv, Predicate, Region,
    SolverInterner, Ty,
};

type I<'s> = SolverInterner<'s>;
/// Owns the inference context used by both type relations and goal evaluation.
#[derive(Clone)]
pub struct Solver<'s>(InferCtxt<'s>);

impl<'s> Deref for Solver<'s> {
    type Target = InferCtxt<'s>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'s> Solver<'s> {
    /// Evaluate one goal and keep useful assignments only when the required facts were available.
    /// An ambiguous answer can still constrain variables. A rejected goal or missing callback
    /// leaves the context as it was before the attempt, through the snapshot's rollback on drop.
    pub fn evaluate(&self, env: ParamEnv<'s>, predicate: Predicate<'s>) -> Outcome {
        use rustc_next_trait_solver::solve::SolverDelegateEvalExt;
        use rustc_type_ir::TypeVisitableExt;
        if self.interner.take_unavailable().is_some()
            || self.interner.is_cancelled()
            || predicate.references_error()
        {
            return Outcome::Unavailable;
        }
        let snapshot = self.snapshot();
        let was_tainted = self.is_tainted();
        let result = self.evaluate_root_goal(Goal::new(self.interner, env, predicate), (), None);
        if let Some(reason) = self.interner.take_unavailable() {
            tracing::debug!(reason, "trait goal unavailable");
            return Outcome::Unavailable;
        }
        if self.interner.is_cancelled() || self.is_tainted() && !was_tainted {
            return Outcome::Unavailable;
        }
        match result {
            Ok(result) => {
                snapshot.commit();
                if result.certainty == Certainty::Yes {
                    Outcome::Proven
                } else {
                    Outcome::Ambiguous
                }
            }
            Err(_) => Outcome::NoSolution,
        }
    }

    pub fn interner(&self) -> I<'s> {
        self.0.interner
    }

    pub fn new(cx: I<'s>) -> Self {
        Self(InferCtxt::new(cx, ir::TypingMode::non_body_analysis()))
    }
}

impl<'s> SolverDelegate for Solver<'s> {
    type Interner = I<'s>;
    type Infcx = InferCtxt<'s>;
    fn build_with_canonical<V: TypeFoldable<I<'s>>>(
        cx: I<'s>,
        input: &ir::CanonicalQueryInput<I<'s>, V>,
    ) -> (Self, V, ir::CanonicalVarValues<I<'s>>) {
        // Canonical slots have no meaning in the caller's variable tables. Give them fresh
        // variables here, preserving which placeholder universes each slot is allowed to name.
        let solver = Self(InferCtxt::new(cx, input.typing_mode.0));
        let canonical = &input.canonical;
        let universes = std::iter::once(solver.universe())
            .chain((1..=canonical.max_universe.as_u32()).map(|_| solver.create_next_universe()))
            .collect::<Vec<_>>();
        let vars =
            ir::CanonicalVarValues::instantiate(cx, canonical.var_kinds, |previous, kind| {
                solver.instantiate_canonical_var(kind, (), previous, |u| universes[u.as_usize()])
            });
        let value = solver.instantiate_canonical(canonical.clone(), vars);
        (solver, value, vars)
    }

    fn compute_goal_fast_path(&self, goal: Goal<I<'s>, Predicate<'s>>, _: ()) -> Option<Certainty> {
        // An unsupported callback invalidates the whole root, including its cached answers.
        // Leave nested obligations undecided until the caller rolls that root back, instead of
        // exploring branches whose results cannot be used.
        if self.interner.has_unavailable() {
            return Some(Certainty::AMBIGUOUS);
        }

        // As in rust-analyzer, wait for receiver evidence before searching `?T: Trait`.
        // Opaque definitions may supply that evidence, so they keep the full solver path.
        if let ir::PredicateKind::Clause(ir::ClauseKind::Trait(predicate)) =
            goal.predicate.kind().skip_binder()
            && self.shallow_resolve(predicate.self_ty()).is_ty_var()
            && self.opaque_types_storage_num_entries() == Default::default()
        {
            return Some(Certainty::AMBIGUOUS);
        }
        None
    }

    fn fresh_var_for_kind_with_span(&self, arg: GenericArg<'s>, _: ()) -> GenericArg<'s> {
        match arg.kind() {
            ir::GenericArgKind::Type(_) => self.next_ty_infer().into(),
            ir::GenericArgKind::Const(_) => self.next_const_infer().into(),
            ir::GenericArgKind::Lifetime(_) => self.next_region_infer().into(),
        }
    }

    // Generalization checks type/const placeholder universes. Region validity, including region
    // leak checking, is omitted; no borrowing verdict is produced by this engine.
    fn leak_check(&self, _: ir::UniverseIndex) -> Result<(), NoSolution> {
        Ok(())
    }

    fn evaluate_const(&self, _: ParamEnv<'s>, _: ir::UnevaluatedConst<I<'s>>) -> Option<Const<'s>> {
        self.interner.unavailable("constant evaluation");
        None
    }

    fn well_formed_goals(
        &self,
        _: ParamEnv<'s>,
        _: super::Term<'s>,
    ) -> Option<Vec<Goal<I<'s>, Predicate<'s>>>> {
        // TODO: Lower well-formedness obligations when source types carry enough information.
        // None stalls the goal, matching rust-analyzer's inference-only implementation.
        None
    }

    fn make_deduplicated_region_constraints(
        &self,
    ) -> Vec<(ir::RegionConstraint<I<'s>>, VisibleForLeakCheck)> {
        Vec::new()
    }

    fn instantiate_canonical<V: TypeFoldable<I<'s>>>(
        &self,
        canonical: ir::Canonical<I<'s>, V>,
        values: ir::CanonicalVarValues<I<'s>>,
    ) -> V {
        assert_eq!(canonical.var_kinds.len(), values.len());
        // Canonical binders are independent of ordinary higher-ranked binders. The canonical
        // variables never become captured when this traversal descends into a function type.
        struct Instantiator<'s> {
            cx: I<'s>,
            values: ir::CanonicalVarValues<I<'s>>,
        }

        impl<'s> TypeFolder<I<'s>> for Instantiator<'s> {
            fn cx(&self) -> I<'s> {
                self.cx
            }

            fn fold_ty(&mut self, t: Ty<'s>) -> Ty<'s> {
                match t.kind() {
                    ir::Bound(ir::BoundVarIndexKind::Canonical, b) => {
                        self.values[b.var].expect_ty()
                    }
                    _ => t.super_fold_with(self),
                }
            }

            fn fold_const(&mut self, c: Const<'s>) -> Const<'s> {
                match c.kind() {
                    ir::ConstKind::Bound(ir::BoundVarIndexKind::Canonical, b) => {
                        self.values[b.var].expect_const()
                    }
                    _ => c.super_fold_with(self),
                }
            }

            fn fold_region(&mut self, r: Region<'s>) -> Region<'s> {
                match r.kind() {
                    ir::ReBound(ir::BoundVarIndexKind::Canonical, b) => {
                        self.values[b.var].expect_region()
                    }
                    _ => r,
                }
            }
        }
        canonical.value.fold_with(&mut Instantiator {
            cx: self.interner,
            values,
        })
    }

    fn instantiate_canonical_var(
        &self,
        kind: ir::CanonicalVarKind<I<'s>>,
        _: (),
        previous: &[GenericArg<'s>],
        map: impl Fn(ir::UniverseIndex) -> ir::UniverseIndex,
    ) -> GenericArg<'s> {
        match kind {
            ir::CanonicalVarKind::Ty { ui, sub_root } => {
                let ty = self.next_ty_var_in_universe(map(ui));
                if let Some(previous) = previous.get(sub_root.as_usize()) {
                    let ir::Infer(ir::TyVar(a)) = ty.kind() else {
                        unreachable!()
                    };
                    let ir::Infer(ir::TyVar(b)) = previous.expect_ty().kind() else {
                        unreachable!()
                    };
                    self.sub_unify_ty_vids_raw(a, b);
                }
                ty.into()
            }
            ir::CanonicalVarKind::Int => self.next_int_var().into(),
            ir::CanonicalVarKind::Float => self.next_float_var().into(),
            ir::CanonicalVarKind::Region(u) => self.next_region_var_in_universe(map(u)).into(),
            ir::CanonicalVarKind::Const(u) => self.next_const_var_in_universe(map(u)).into(),
            ir::CanonicalVarKind::PlaceholderTy(p) => Ty::new_placeholder(
                self.interner,
                ir::PlaceholderType::new(map(p.universe), p.bound),
            )
            .into(),
            ir::CanonicalVarKind::PlaceholderRegion(p) => Region::new_placeholder(
                self.interner,
                ir::PlaceholderRegion::new(map(p.universe), p.bound),
            )
            .into(),
            ir::CanonicalVarKind::PlaceholderConst(p) => Const::new_placeholder(
                self.interner,
                ir::PlaceholderConst::new(map(p.universe), p.bound),
            )
            .into(),
        }
    }

    fn add_item_bounds_for_hidden_type(
        &self,
        id: DefId,
        args: GenericArgs<'s>,
        env: ParamEnv<'s>,
        hidden: Ty<'s>,
        goals: &mut Vec<Goal<I<'s>, Predicate<'s>>>,
    ) {
        struct ReplaceOpaque<'s> {
            cx: I<'s>,
            id: DefId,
            args: GenericArgs<'s>,
            hidden: Ty<'s>,
        }

        impl<'s> TypeFolder<I<'s>> for ReplaceOpaque<'s> {
            fn cx(&self) -> I<'s> {
                self.cx
            }

            fn fold_ty(&mut self, t: Ty<'s>) -> Ty<'s> {
                if let ir::Alias(alias) = t.kind()
                    && alias.kind.def_id() == self.id
                    && alias.args == self.args
                {
                    self.hidden
                } else {
                    t.super_fold_with(self)
                }
            }
        }
        let bounds = self
            .interner
            .item_bounds(id)
            .iter_instantiated(self.interner, args);
        for bound in bounds {
            goals.push(Goal::new(
                self.interner,
                env,
                bound.skip_norm_wip().fold_with(&mut ReplaceOpaque {
                    cx: self.interner,
                    id,
                    args,
                    hidden,
                }),
            ));
        }

        // No region validity or general well-formedness diagnostics are requested here.
    }

    fn fetch_eligible_assoc_item(
        &self,
        _: ir::TraitRef<I<'s>>,
        item: DefId,
        implementation: DefId,
    ) -> FetchEligibleAssocItemResponse<I<'s>> {
        let item_data = self.interner.declaration(item);
        let impl_data = self.interner.declaration(implementation);
        if let DeclarationKind::Impl {
            associated_types, ..
        } = &impl_data.kind
        {
            if let Some((_, id)) = associated_types
                .iter()
                .find(|(name, _)| *name == item_data.name)
            {
                return FetchEligibleAssocItemResponse::Found(*id);
            }
            if matches!(&item_data.kind, DeclarationKind::Alias(Some(_))) {
                return FetchEligibleAssocItemResponse::Found(item);
            }
        }
        self.interner.unavailable("associated item implementation");
        FetchEligibleAssocItemResponse::Err(super::types::ErrorGuaranteed)
    }

    fn is_transmutable(&self, _: Ty<'s>, _: Ty<'s>, _: Const<'s>) -> Result<Certainty, NoSolution> {
        self.interner.unavailable("transmutability");
        Ok(Certainty::AMBIGUOUS)
    }
}

/// What we learned from trying a goal, including whether we had enough source information.
///
/// Missing a declaration or an unsupported compiler callback is different from proving that
/// no impl applies. Callers must leave those questions open instead of ruling out the Rust code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The goal holds with the assignments made during evaluation.
    Proven,
    /// The solver could proceed, but the goal still needs evidence, such as the type of `?T`.
    Ambiguous,
    /// The solver had the required information and rejected the goal.
    NoSolution,
    /// Evaluation used missing or unsupported information, or was cancelled; discard its answer.
    Unavailable,
}
