//! Semantic operations on a body's live inference values.
//!
//! Lookup supplies visible declarations. This layer checks whether they fit the caller's types
//! and tries to prove their bounds. For example, matching `impl<T> Trait for Vec<T>` against
//! `Vec<?Item>` connects the impl's `T` to the body's still-unknown `?Item`.
//!
//! A candidate keeps its own trial table. Looking at another candidate must not inherit the
//! assignments made while checking the first one. The caller can adopt the chosen table, or
//! convert its result to owned types before dropping the solver.

use rg_ir_model::{FunctionRef, GenericParamRef, ImplRef, TraitDefRef};
use rg_item_tree::FunctionQualifiers;
use rg_std::{ExpectedUnique, UniqueVec};
use rustc_type_ir::{
    self as ir, Interner as _, TypeVisitableExt as _, Upcast, inherent::IntoKind as _,
    lang_items::SolverTraitLangItem,
};

use super::{
    AliasTy, Clause, DefId, FnDefTy, GenericArgs, InferVarKind, InferenceSubstitution,
    InferenceTable, List, Outcome, ProjectionTy, SolverInterner, Ty, TyShape,
    declarations::LangItem,
};
use crate::{FnDefRef, Ty as StoredTy, signature};

/// A trait together with its live arguments. `args[0]` is `Self`: for `Vec<?T>: IntoIterator`,
/// it holds `Vec<?T>`. Associated-type equalities such as `Item = u8` are separate clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitApplication<'s> {
    pub def: TraitDefRef,
    pub args: GenericArgs<'s>,
}

impl<'s> TraitApplication<'s> {
    pub fn self_ty(self) -> Option<Ty<'s>> {
        self.args.first().and_then(|arg| arg.as_ty())
    }

    pub fn raise(self, cx: SolverInterner<'s>) -> crate::TraitApplication {
        crate::TraitApplication {
            def: self.def,
            args: cx.raise_args(self.args),
        }
    }

    pub fn clause(self, cx: SolverInterner<'s>) -> Clause<'s> {
        let def = DefId::Trait(self.def);
        ir::TraitRef::new_from_args(cx, def, cx.complete_args(def, self.args)).upcast(cx)
    }
}

/// A signature in the operation's storage. The declaration of `fn id<T>(x: T) -> T` keeps
/// `T` in both positions; instantiating it for a call replaces both with the same live `?T`.
#[derive(Debug, Clone, Copy)]
pub struct CallableSignature<'s> {
    pub params: List<'s, Ty<'s>>,
    /// The type of the call expression. For an async function this is the future.
    pub call_return: Ty<'s>,
    pub(crate) async_output: Option<Ty<'s>>,
    pub qualifiers: FunctionQualifiers,
}

impl<'s> CallableSignature<'s> {
    /// The type written after `->`, or unit when the declaration omits it.
    pub fn body_output(self) -> Ty<'s> {
        self.async_output.unwrap_or(self.call_return)
    }

    pub fn raise(self, cx: SolverInterner<'s>) -> signature::CallableSignature {
        signature::CallableSignature {
            params: self
                .params
                .iter()
                .map(|ty| cx.raise_ty(ty).unwrap_or(StoredTy::Unknown))
                .collect(),
            call_return: cx.raise_ty(self.call_return).unwrap_or(StoredTy::Unknown),
            async_output: self
                .async_output
                .map(|ty| Box::new(cx.raise_ty(ty).unwrap_or(StoredTy::Unknown))),
            qualifiers: self.qualifiers,
        }
    }
}

impl<'s> FnDefTy<'s> {
    /// Use the arguments fixed when this function item was formed. A later `f(...)` can
    /// refine those variables, and another use of the same binding sees that refinement.
    pub fn signature(self, cx: SolverInterner<'s>) -> Option<CallableSignature<'s>> {
        let signature = cx.callable_signature(self.def)?;
        let params = cx.params(self.def.into());
        if params.is_empty() {
            return Some(signature);
        }
        let subst = InferenceSubstitution::from_args(params.iter().copied(), self.args);
        Some(CallableSignature {
            params: subst.apply(cx, signature.params),
            call_return: subst.apply(cx, signature.call_return),
            async_output: signature.async_output.map(|ty| subst.apply(cx, ty)),
            qualifiers: signature.qualifiers,
        })
    }

    /// Reify a tuple constructor at a coercion site, keeping its live generic arguments.
    pub fn constructor_pointer(self, cx: SolverInterner<'s>) -> Option<Ty<'s>> {
        // TODO: Preserve safety and ABI in pointer types before reifying named functions.
        if !matches!(self.def, FnDefRef::Constructor(_)) {
            return None;
        }
        let signature = self.signature(cx)?;
        Some(cx.fn_pointer(&signature.params, signature.call_return))
    }
}

/// A source bound keeps its associated equalities beside the positional trait arguments.
/// Once registered with inference, these become ordinary compiler clauses.
#[derive(Debug, Clone)]
pub struct TraitRefLowering<'s> {
    pub application: TraitApplication<'s>,
    pub associated_types: Vec<AssocTypeBinding<'s>>,
}

impl<'s> TraitRefLowering<'s> {
    pub fn clauses(&self, cx: SolverInterner<'s>) -> impl Iterator<Item = Clause<'s>> + '_ {
        std::iter::once(self.application.clause(cx)).chain(
            self.associated_types
                .iter()
                .map(move |binding| binding.clause(cx)),
        )
    }

    pub fn raise(&self, cx: SolverInterner<'s>) -> crate::TraitRefLowering {
        crate::TraitRefLowering {
            application: self.application.raise(cx),
            associated_types: self
                .associated_types
                .iter()
                .map(|binding| binding.raise(cx))
                .collect(),
        }
    }
}

/// An equality for one complete projection. Its arguments belong to the trait that declares
/// the associated type, including any substitutions made while following supertraits.
#[derive(Debug, Clone, Copy)]
pub struct AssocTypeBinding<'s> {
    pub projection: ProjectionTy<'s>,
    pub ty: Ty<'s>,
}

impl<'s> AssocTypeBinding<'s> {
    pub fn clause(self, cx: SolverInterner<'s>) -> Clause<'s> {
        let def_id = DefId::TypeAlias(self.projection.associated_ty);
        ir::ProjectionPredicate {
            projection_term: ir::AliasTerm::new_from_args(
                cx,
                ir::AliasTermKind::ProjectionTy { def_id },
                cx.complete_args(def_id, self.projection.args),
            ),
            term: self.ty.into(),
        }
        .upcast(cx)
    }

    pub fn raise(self, cx: SolverInterner<'s>) -> crate::AssocTypeBinding {
        crate::AssocTypeBinding {
            projection: crate::ProjectionTy {
                associated_ty: self.projection.associated_ty,
                args: cx.raise_args(self.projection.args),
            },
            ty: cx.raise_ty(self.ty).unwrap_or(crate::Ty::Unknown),
        }
    }
}

/// An impl's receiver and optional trait application, with its declaration parameters still
/// generic. Matching the impl to a receiver supplies arguments for those parameters; checking
/// its requirements is a separate operation.
#[derive(Debug, Clone, Copy)]
pub struct ImplHeader<'s> {
    pub owner: ImplRef,
    pub self_ty: Ty<'s>,
    pub trait_ref: Option<TraitApplication<'s>>,
}

impl<'s> ImplHeader<'s> {
    pub fn raise(self, cx: SolverInterner<'s>) -> signature::ImplHeader {
        signature::ImplHeader {
            owner: self.owner,
            self_ty: cx.raise_ty(self.self_ty).unwrap_or(crate::Ty::Unknown),
            trait_ref: self.trait_ref.map(|application| application.raise(cx)),
        }
    }
}

/// The substitution and trial table learned from matching an impl's receiver and trait arguments.
/// Associated-item discovery can use this substitution before the impl's requirements are read.
/// Matching projection types may also queue goals, which full selection still needs to solve.
pub(crate) struct ImplHeaderMatch<'s> {
    pub application: Option<TraitApplication<'s>>,
    pub subst: InferenceSubstitution<'s>,
    pub table: InferenceTable<'s>,
    available: bool,
}

/// Everything learned while checking one impl, including the trial table that owns its variables.
/// A result can still have pending or unsupported bounds; `outcome` says how much was proved.
/// Dropping this value rejects the trial. Adopting its table keeps its assignments in the caller.
#[derive(Clone)]
pub struct ImplSelection<'s> {
    pub impl_ref: ImplRef,
    pub application: Option<TraitApplication<'s>>,
    pub subst: InferenceSubstitution<'s>,
    pub outcome: Outcome,
    pub table: InferenceTable<'s>,
}

impl<'s> InferenceTable<'s> {
    /// Recover a callable bound's arguments and project its output. For `F: Fn(u8) -> R`,
    /// proving `F: FnOnce<(?Arg,)>` learns `?Arg = u8` before the body visits the argument.
    pub fn callable_bound_signature(
        &self,
        callee: Ty<'s>,
        arguments: impl Iterator<Item = Ty<'s>> + Clone,
    ) -> Option<(List<'s, Ty<'s>>, Ty<'s>)> {
        let cx = self.interner();
        let DefId::Trait(def) = cx.lang_item(LangItem::Trait(SolverTraitLangItem::FnOnce))? else {
            return None;
        };
        let DefId::TypeAlias(associated_ty) = cx.lang_item(LangItem::FnOnceOutput)? else {
            return None;
        };
        let probe = |use_arguments| {
            self.commit_if_some(|table| -> Result<_, std::convert::Infallible> {
                if cx.metadata(DefId::TypeAlias(associated_ty)).parent != Some(DefId::Trait(def)) {
                    return Ok(None);
                }
                let params = arguments
                    .clone()
                    .map(|arg| {
                        if use_arguments && !arg.is_never() {
                            table.instantiate_unknowns(arg)
                        } else {
                            table.new_type_var()
                        }
                    })
                    .collect::<Vec<_>>();
                let args = List::new(cx, &[callee.into(), cx.tuple(&params).into()]);
                if table.prove([TraitApplication { def, args }.clause(cx)]) != Outcome::Proven {
                    return Ok(None);
                }
                let output = cx.projection(ProjectionTy {
                    associated_ty,
                    args,
                });
                Ok(Some((List::new(cx, &params), output)))
            })
            .expect("callable bound probe is infallible")
        };

        // Prefer the signature from the bound itself so its parameters can guide coercions.
        // When `F` has both `Fn(u8)` and `Fn(u16)`, a typed argument can break the tie.
        probe(false).or_else(|| {
            let has_argument_evidence = arguments.clone().any(|arg| {
                !matches!(
                    arg.shape(),
                    TyShape::Unknown
                        | TyShape::Never
                        | TyShape::InferVar {
                            kind: InferVarKind::Type
                        }
                )
            });
            if has_argument_evidence {
                probe(true)
            } else {
                None
            }
        })
    }

    pub(crate) fn has_trait_assumption(&self, trait_ref: TraitDefRef) -> bool {
        self.environment().clauses.iter().any(|clause| {
            matches!(
                clause.kind().skip_binder(),
                ir::ClauseKind::Trait(predicate) if predicate.trait_ref.def_id == DefId::Trait(trait_ref)
            )
        })
    }

    /// Traits supplied by bounds on this receiver are method candidates even when the trait is
    /// named only in a bound, such as `fn draw<T: api::Render>(value: T)`. The environment already
    /// includes supertraits. Objects carry bounds in their type; opaque and associated types
    /// carry bounds on their declarations.
    pub fn bound_traits(&self, receiver: Ty<'s>) -> UniqueVec<TraitDefRef> {
        let cx = self.interner();
        let receiver = self.resolve_root_var(receiver);
        let mut traits = UniqueVec::new();
        let mut collect = |clause: Clause<'s>| {
            if let ir::ClauseKind::Trait(predicate) = clause.kind().skip_binder()
                && self.resolve_root_var(predicate.trait_ref.self_ty()) == receiver
                && let DefId::Trait(trait_ref) = predicate.trait_ref.def_id
            {
                traits.push(trait_ref);
            }
        };
        for clause in self.environment().clauses {
            collect(clause);
        }
        if let TyShape::Dyn(object) = receiver.shape() {
            for clause in ir::elaborate::elaborate(cx, object.clauses(cx, receiver)) {
                collect(clause);
            }
        }
        let alias = match receiver.shape() {
            TyShape::Alias(AliasTy::Opaque(alias)) => {
                Some((DefId::Opaque(alias.opaque), alias.args))
            }
            TyShape::Alias(AliasTy::Projection(alias)) => {
                Some((DefId::TypeAlias(alias.associated_ty), alias.args))
            }
            _ => None,
        };
        if let Some((owner, args)) = alias {
            let subst = InferenceSubstitution::from_args(cx.params(owner).iter().copied(), args);
            let bounds = subst.apply(cx, cx.bounds(owner));
            for clause in ir::elaborate::elaborate(cx, bounds) {
                collect(clause);
            }
        }
        traits
    }

    /// Give each declaration parameter a fresh variable for this use of the declaration.
    /// Two calls to `id<T>` need separate `?T`s so their argument types can differ.
    pub fn fresh_substitution(&self, owner: DefId) -> InferenceSubstitution<'s> {
        let mut subst = InferenceSubstitution::new();
        subst.fresh_for(self, self.params(owner).iter().copied());
        subst
    }

    /// Completion knows the receiver but has no call arguments yet. For
    /// `fn draw<T: Paint>(&self, brush: T) where Self: Canvas`, check `Self: Canvas`
    /// now and leave `T: Paint` for the call.
    pub fn method_receiver_bounds_hold(
        &self,
        function: FunctionRef,
        mut subst: InferenceSubstitution<'s>,
    ) -> bool {
        let cx = self.interner();
        let callbacks = cx.track_callbacks();
        let owner = DefId::Function(function);
        subst.fresh_for(self, self.params(owner).iter().copied());
        let mut remaining: Vec<_> = subst.apply(cx, cx.predicates(owner)).iter().collect();
        if callbacks.failure().is_some() {
            return false;
        }

        loop {
            let before = remaining.len();
            remaining.retain(|&clause| {
                let clause = self.resolve(clause);
                // In `Self: Source<Item = T>`, knowing Self can tell us T. Queue this
                // equality now so the next pass can check `T: Paint`. In contrast,
                // `Item = T::Item` still needs the caller to choose T.
                let known = !clause.has_non_region_infer()
                    || matches!(clause.kind().skip_binder(), ir::ClauseKind::Projection(eq)
                        if !eq.projection_term.args.has_non_region_infer() && !eq.term.has_aliases());
                if known {
                    self.register(clause);
                }
                !known
            });
            if self.fulfill() != Outcome::Proven {
                return false;
            }
            if remaining.is_empty() || remaining.len() == before {
                return true;
            }
        }
    }

    /// Form a function item and register its requirements with the same substitution.
    ///
    /// For `fn copy<T: Clone>(value: T) -> T` with `T = ?T`, the parameter, return type, and queued
    /// `?T: Clone` goal all share that variable. Learning the argument type then gives the bound
    /// the same evidence without rebuilding the signature.
    pub fn instantiate_function_item(
        &self,
        function: FunctionRef,
        subst: &InferenceSubstitution<'s>,
    ) -> Option<FnDefTy<'s>> {
        let cx = self.interner();
        let callbacks = cx.track_callbacks();
        cx.callable_signature(function)?;
        let owner = DefId::Function(function);
        let args = subst.args_for(cx, cx.params(owner).iter().copied());
        let clauses = subst.apply(cx, cx.predicates(owner));
        if callbacks.failure().is_some() {
            return None;
        }
        for clause in clauses {
            self.register(clause);
        }
        Some(FnDefTy {
            def: function.into(),
            args,
        })
    }

    /// Match an impl's receiver and optional trait arguments without loading its bounds.
    /// Name lookup uses this while lowering bounds that mention the impl's associated types.
    ///
    /// For `impl<T: Clone> Trait for Vec<T>` and receiver `Vec<u8>`, matching learns `T = u8`
    /// without reading `T: Clone`. Applicability requests that bound only after the header fits.
    pub(crate) fn match_impl_header(
        &self,
        impl_ref: ImplRef,
        receiver: Ty<'s>,
        expected: Option<TraitApplication<'s>>,
    ) -> Option<ImplHeaderMatch<'s>> {
        let cx = self.interner();
        // Loading the header is part of checking this candidate. It can report missing data
        // and make us return None below, before we have a trial table or goals to fulfill.
        // Include those reads in this scope; the next candidate gets its own starting count.
        let callbacks = cx.track_callbacks();
        // Negative impls still participate in solver proof, but cannot supply methods or
        // associated types as if they were an implementation of the trait.
        if cx.impl_polarity(DefId::Impl(impl_ref)) == ir::ImplPolarity::Negative {
            return None;
        }
        let header = cx.impl_header(impl_ref)?;
        // An unreadable inherent impl header gives us no receiver to match. Treating it as
        // a wildcard would offer all its methods on every type. Partly known headers such as
        // `Container<Unknown>` still have a useful outer type and continue through matching.
        if header.trait_ref.is_none() && header.self_ty.is_unknown() {
            return None;
        }
        // Instantiate the impl in a separate trial. Its generic parameters must be free to learn
        // from the receiver without changing the caller's table if this candidate is rejected.
        let table = self.probe();
        let subst = table.fresh_substitution(DefId::Impl(impl_ref));
        let self_ty = subst.apply(cx, header.self_ty);
        if table.try_unify(receiver, self_ty).is_err() {
            return None;
        }
        let application = header.trait_ref.as_ref().map(|tr| TraitApplication {
            def: tr.def,
            args: subst.apply(cx, tr.args),
        });
        // A receiver match is enough for inherent lookup. A named trait goal also supplies
        // arguments: matching `Convert<u8>` must not accept an impl of `Convert<u16>`.
        if let Some(expected) = expected {
            let actual = application?;
            if actual.def != expected.def
                || table.try_unify_args(actual.args, expected.args).is_err()
            {
                return None;
            }
        }
        // Matching the readable parts can still help discovery. Carry completeness separately
        // from proof: an incomplete receiver is not enough evidence to accept this impl later.
        let available =
            callbacks.failure().is_none() && !receiver.has_unknown() && !self_ty.has_unknown();
        Some(ImplHeaderMatch {
            application,
            subst,
            table,
            available,
        })
    }

    /// Match one impl, then load its requirements and try to prove them with the matched arguments.
    /// A proved failure rejects the impl; an unfinished proof remains a possible candidate so
    /// editor lookup can still use its declarations.
    pub fn select_impl(
        &self,
        impl_ref: ImplRef,
        receiver: Ty<'s>,
        expected: Option<TraitApplication<'s>>,
    ) -> Option<ImplSelection<'s>> {
        let cx = self.interner();
        let callbacks = cx.track_callbacks();
        let matched = self.match_impl_header(impl_ref, receiver, expected)?;
        // For `impl<T: Clone> Trait for Vec<T>` matched to `Vec<u8>`, this queues `u8: Clone`.
        // If a bound names an associated alias, resolving it uses header matching and does not
        // request these predicates again.
        let clauses = cx.predicates(DefId::Impl(impl_ref));
        for clause in clauses {
            matched.table.register(matched.subst.apply(cx, clause));
        }
        let complete = matched.available && callbacks.failure().is_none();
        let outcome = matched.table.fulfill();
        if complete && outcome == Outcome::NoSolution {
            return None;
        }
        Some(ImplSelection {
            impl_ref,
            application: matched.application,
            subst: matched.subst,
            outcome: if complete {
                outcome
            } else {
                Outcome::Unavailable
            },
            table: matched.table,
        })
    }

    /// Try each source impl in its own table so competing candidates cannot constrain each
    /// other. Proven candidates take precedence over possible ones, but distinct impls remain
    /// ambiguous even if they infer the same types.
    pub fn select_trait_impl(
        &self,
        application: TraitApplication<'s>,
        bindings: &[Clause<'s>],
        candidates: impl IntoIterator<Item = ImplRef>,
    ) -> ExpectedUnique<ImplSelection<'s>> {
        let receiver = application.args[0].as_ty().expect("Self is a type");
        let mut proven = ExpectedUnique::new();
        let mut possible = ExpectedUnique::new();
        for candidate in candidates {
            let Some(mut selected) = self.select_impl(candidate, receiver, Some(application))
            else {
                continue;
            };
            for &binding in bindings {
                selected.table.register(binding);
            }
            let outcome = selected.table.fulfill();
            if outcome == Outcome::NoSolution {
                continue;
            }
            if selected.outcome == Outcome::Proven {
                selected.outcome = outcome;
            }
            let rank = if selected.outcome == Outcome::Proven {
                &mut proven
            } else {
                &mut possible
            };
            match rank {
                ExpectedUnique::Empty => *rank = ExpectedUnique::One(selected),
                ExpectedUnique::One(previous) if previous.impl_ref == selected.impl_ref => {}
                ExpectedUnique::One(_) | ExpectedUnique::Ambiguous => {
                    *rank = ExpectedUnique::Ambiguous;
                }
            }
        }
        if proven.is_empty() { possible } else { proven }
    }

    /// Normalize the projection together with its trait and associated-type requirements.
    /// Keep the assignments in this table so callers can inspect inference variables afterward.
    /// For `<Iter<?T> as Iterator>::Item`, the answer can still be the live variable `?T`.
    pub fn normalize_assoc_type(
        &self,
        application: TraitApplication<'s>,
        bindings: &[Clause<'s>],
        projection: ProjectionTy<'s>,
    ) -> Option<(Ty<'s>, Outcome)> {
        let cx = self.interner();
        let callbacks = cx.track_callbacks();
        self.register(application.clause(cx));
        for &binding in bindings {
            self.register(binding);
        }
        let ty = self.normalize(cx.projection(projection));
        // Preparing the projection can read declarations before any goal is evaluated. Check
        // those reads here: fulfillment's per-goal scopes only see failures during that goal.
        if callbacks.failure().is_some() {
            return None;
        }
        match self.fulfill() {
            outcome @ (Outcome::Proven | Outcome::Ambiguous) => Some((ty, outcome)),
            Outcome::NoSolution | Outcome::Unavailable => None,
        }
    }

    /// The caller has selected exactly one candidate. Keep body obligations while adopting the
    /// candidate's assignments; its fresh variables were allocated after the body's existing IDs.
    pub fn adopt(&mut self, mut candidate: Self) {
        let obligations = std::mem::take(self.pending.get_mut());
        candidate.pending.get_mut().extend(obligations);
        *self = candidate;
    }
}

impl<'s> InferenceSubstitution<'s> {
    /// Export the learned arguments for one declaration so they can outlive this table.
    /// Following the table's assignments turns `T = ?T` with `?T = u8` into owned `T = u8`.
    pub(crate) fn finalize(&self, table: &InferenceTable<'s>, owner: DefId) -> crate::Substitution {
        let mut subst = crate::Substitution::new();
        for &param in table.params(owner) {
            if let Some(arg) = self.get(param) {
                let args = table.finalize_args(List::new(table.interner(), &[arg]));
                subst.push(param, args[0].clone());
            }
        }
        subst
    }

    pub fn from_args(
        params: impl IntoIterator<Item = GenericParamRef>,
        args: GenericArgs<'s>,
    ) -> Self {
        let mut subst = Self::new();
        for (param, arg) in params.into_iter().zip(args) {
            subst.insert(param, arg);
        }
        subst
    }

    pub fn extend(&mut self, other: Self) {
        self.args.extend(other.args);
    }
}
