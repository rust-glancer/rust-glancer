//! Semantic operations on a body's live inference values.
//!
//! Lookup supplies visible declarations. This layer checks whether they fit the caller's types
//! and tries to prove their bounds. For example, matching `impl<T> Trait for Vec<T>` against
//! `Vec<?Item>` connects the impl's `T` to the body's still-unknown `?Item`.
//!
//! A candidate keeps its own trial table. Looking at another candidate must not inherit the
//! assignments made while checking the first one. The caller can adopt the chosen table, or
//! convert its result to owned types before dropping the solver.

use rg_ir_model::{FunctionRef, GenericParamRef, ImplRef, TraitDefRef, TypeAliasRef};
use rg_item_tree::FunctionQualifiers;
use rg_std::ExpectedUnique;
use rustc_type_ir::{self as ir, Upcast};

use super::{
    Clause, DeclarationKind, DefId, GenericArgs, InferenceSubstitution, InferenceTable, List,
    Outcome, ProjectionTy, SolverInterner, Ty,
};
use crate::signature;

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

    /// Turn a binding such as `Iterator<Item = u8>` into `<Self as Iterator>::Item = u8`.
    pub fn associated_type_eq(
        self,
        cx: SolverInterner<'s>,
        associated_ty: TypeAliasRef,
        ty: Ty<'s>,
    ) -> Clause<'s> {
        let def_id = DefId::TypeAlias(associated_ty);
        ir::ProjectionPredicate {
            projection_term: ir::AliasTerm::new_from_args(
                cx,
                ir::AliasTermKind::ProjectionTy { def_id },
                cx.complete_args(def_id, self.args),
            ),
            term: ty.into(),
        }
        .upcast(cx)
    }
}

/// A signature in the operation's storage. The declaration of `fn id<T>(x: T) -> T` keeps
/// `T` in both positions; instantiating it for a call replaces both with the same live `?T`.
#[derive(Debug, Clone, Copy)]
pub struct CallableSignature<'s> {
    pub params: List<'s, Ty<'s>>,
    pub ret: Ty<'s>,
    pub clauses: List<'s, Clause<'s>>,
    pub qualifiers: FunctionQualifiers,
}

impl<'s> CallableSignature<'s> {
    pub fn raise(self, cx: SolverInterner<'s>) -> signature::CallableSignature {
        signature::CallableSignature {
            params: self
                .params
                .iter()
                .map(|ty| cx.raise_ty(ty).unwrap_or(crate::Ty::Unknown))
                .collect(),
            ret: cx.raise_ty(self.ret).unwrap_or(crate::Ty::Unknown),
            clauses: self
                .clauses
                .iter()
                .map(|clause| cx.raise_clause(clause))
                .collect(),
            qualifiers: self.qualifiers,
        }
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
        std::iter::once(self.application.clause(cx)).chain(self.associated_types.iter().map(
            move |binding| {
                self.application
                    .associated_type_eq(cx, binding.associated_ty, binding.ty)
            },
        ))
    }

    pub fn raise(&self, cx: SolverInterner<'s>) -> crate::TraitRefLowering {
        crate::TraitRefLowering {
            application: self.application.raise(cx),
            associated_types: self
                .associated_types
                .iter()
                .map(|b| crate::AssocTypeBinding {
                    associated_ty: b.associated_ty,
                    ty: cx.raise_ty(b.ty).unwrap_or(crate::Ty::Unknown),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AssocTypeBinding<'s> {
    pub associated_ty: TypeAliasRef,
    pub ty: Ty<'s>,
}

/// Declaration parameters remain in this template until an impl is matched to a receiver.
#[derive(Debug, Clone)]
pub struct ImplHeader<'s> {
    pub owner: ImplRef,
    pub self_ty: Ty<'s>,
    pub trait_ref: Option<TraitRefLowering<'s>>,
    pub clauses: Vec<Clause<'s>>,
}

impl<'s> ImplHeader<'s> {
    pub fn raise(&self, cx: SolverInterner<'s>) -> signature::ImplHeader {
        signature::ImplHeader {
            owner: self.owner,
            self_ty: cx.raise_ty(self.self_ty).unwrap_or(crate::Ty::Unknown),
            trait_ref: self.trait_ref.as_ref().map(|bound| bound.raise(cx)),
            clauses: self
                .clauses
                .iter()
                .map(|&clause| cx.raise_clause(clause))
                .collect(),
        }
    }
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

impl<'s> SolverInterner<'s> {
    /// The declaration template is shared inside this operation. Reading the enclosing body's
    /// signature retains its parameters; a call instantiates them through `InferenceTable`.
    ///
    /// A declaration read can return fallback data after a nested lookup fails. In that case
    /// return no signature, so call inference cannot use those fallback types as real constraints.
    pub fn function_signature(self, function: FunctionRef) -> Option<CallableSignature<'s>> {
        let callbacks = self.track_callbacks();
        let declaration = self.declaration(DefId::Function(function));
        if callbacks.failure().is_some() {
            return None;
        }
        match &declaration.kind {
            DeclarationKind::Function(signature) => Some(*signature),
            _ => None,
        }
    }
}

impl<'s> InferenceTable<'s> {
    /// Give each declaration parameter a fresh variable for this use of the declaration.
    /// Two calls to `id<T>` need separate `?T`s so their argument types can differ.
    pub fn fresh_substitution(&self, owner: DefId) -> InferenceSubstitution<'s> {
        let mut subst = InferenceSubstitution::new();
        subst.fresh_for(self, self.params(owner).iter().copied());
        subst
    }

    /// Apply one substitution to parameters, return type, and bounds together. A later argument
    /// can then constrain both the return type and a bound such as `T: Clone` through the same `?T`.
    pub fn signature(
        &self,
        function: FunctionRef,
        subst: &InferenceSubstitution<'s>,
    ) -> Option<CallableSignature<'s>> {
        let cx = self.interner();
        let signature = cx.function_signature(function)?;
        Some(CallableSignature {
            params: subst.apply(cx, signature.params),
            ret: subst.apply(cx, signature.ret),
            clauses: subst.apply(cx, signature.clauses),
            qualifiers: signature.qualifiers,
        })
    }

    /// Match a declaration header before proving its predicates. Source-name discovery needs
    /// this stage while it is still lowering the very bounds that will form the environment.
    ///
    /// For `impl<T: Clone> Trait for Vec<T>` and receiver `Vec<u8>`, matching learns `T = u8`
    /// and queues `u8: Clone`. A successful header match alone does not prove that bound.
    pub fn match_impl(
        &self,
        impl_ref: ImplRef,
        receiver: Ty<'s>,
        expected: Option<TraitApplication<'s>>,
    ) -> Option<ImplSelection<'s>> {
        let cx = self.interner();
        // Loading the header is part of checking this candidate. It can report missing data
        // and make us return None below, before we have a trial table or goals to fulfill.
        // Include those reads in this scope; the next candidate gets its own starting count.
        let callbacks = cx.track_callbacks();
        let declaration = cx.declaration(DefId::Impl(impl_ref));
        let DeclarationKind::Impl { header, .. } = &declaration.kind else {
            return None;
        };
        // Instantiate the impl in a separate trial. Its generic parameters must be free to learn
        // from the receiver without changing the body if this candidate is later rejected.
        let table = self.probe();
        let subst = table.fresh_substitution(DefId::Impl(impl_ref));
        let self_ty = subst.apply(cx, header.self_ty);
        if table.try_unify(receiver, self_ty).is_err() {
            return None;
        }
        let application = header.trait_ref.as_ref().map(|tr| TraitApplication {
            def: tr.application.def,
            args: subst.apply(cx, tr.application.args),
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
        let clauses = declaration.predicates.iter().map(|&c| subst.apply(cx, c));
        for clause in clauses {
            table.register(clause);
        }
        let mut outcome = Outcome::Proven;
        // Matching the parts we could read is useful for discovery, but does not make an
        // incomplete header reliable. Carry that distinction with the returned candidate.
        if callbacks.failure().is_some() || receiver.has_unknown() || self_ty.has_unknown() {
            outcome = Outcome::Unavailable;
        }
        Some(ImplSelection {
            impl_ref,
            application,
            subst,
            outcome,
            table,
        })
    }

    /// Match one impl and try its queued bounds. A proved failure rejects the impl; an unfinished
    /// proof remains a possible candidate so editor lookup can still use its declarations.
    pub fn select_impl(
        &self,
        impl_ref: ImplRef,
        receiver: Ty<'s>,
        expected: Option<TraitApplication<'s>>,
    ) -> Option<ImplSelection<'s>> {
        let mut selected = self.match_impl(impl_ref, receiver, expected)?;
        let outcome = selected.table.fulfill();
        if outcome == Outcome::NoSolution {
            return None;
        }
        if selected.outcome == Outcome::Proven {
            selected.outcome = outcome;
        }
        Some(selected)
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
        associated_ty: TypeAliasRef,
    ) -> Option<(Ty<'s>, Outcome)> {
        let cx = self.interner();
        let callbacks = cx.track_callbacks();
        self.register(application.clause(cx));
        for &binding in bindings {
            self.register(binding);
        }
        let ty = self.normalize(cx.projection(ProjectionTy {
            associated_ty,
            args: application.args,
        }));
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
