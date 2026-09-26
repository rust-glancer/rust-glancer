//! Owned query results over the same compiler solver used by body inference.
//!
//! Editor queries start from finalized types. Each operation lowers those inputs, proves or
//! normalizes them in a scoped table, and freezes its result before releasing all solver storage.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, ImplRef, TraitApplicability, TraitImplRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::TraitGoal;
use crate::{
    Substitution, TraitApplication, Ty, TyContext,
    lookup::{ItemPathQuery, TraitImplFilter},
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{self, DefId, InferenceTable, Outcome, SemanticDeclarations, SolverScope},
};

/// A discovered source impl with the substitutions established while proving that exact impl.
/// Ambiguous or unavailable proof stays a candidate and is never reported as established proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub(crate) application: TraitApplication,
    pub subst: Substitution,
    pub applicability: TraitApplicability,
}

impl TraitSelection {
    pub fn application(&self) -> &TraitApplication {
        &self.application
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocProjectionResult {
    pub ty: Ty,
    pub applicability: TraitApplicability,
}

pub(crate) struct SelectedImpl {
    pub subst: Substitution,
    pub application: Option<TraitApplication>,
    pub applicability: TraitApplicability,
}

impl SelectedImpl {
    fn freeze(selection: solver::ImplSelection<'_>) -> Self {
        let table = &selection.table;
        let subst = selection
            .subst
            .finalize(table, DefId::Impl(selection.impl_ref));
        Self {
            subst,
            application: selection.application.map(|tr| TraitApplication {
                def: tr.def,
                args: table.finalize_args(tr.args),
            }),
            applicability: Self::applicability(selection.outcome),
        }
    }

    fn applicability(outcome: Outcome) -> TraitApplicability {
        match outcome {
            Outcome::Proven => TraitApplicability::Yes,
            Outcome::NoSolution => TraitApplicability::No,
            Outcome::Ambiguous | Outcome::Unavailable => TraitApplicability::Maybe,
        }
    }
}

/// Answer trait questions for callers that already have owned types, such as editor lookups.
///
/// Each question gets a temporary inference table and returns an owned answer. The live matching
/// and normalization operations are shared with body inference, but variables cannot be carried
/// from one call to this query into another. A body keeps its table instead, while it gathers
/// evidence from several expressions.
pub struct TraitSelectionQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    context: TyContext<'query, D, I>,
    resolver: R,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error> + Clone,
    I: ItemStoreSource<'query> + Clone,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        let resolver = context.item_paths().clone();
        Self { context, resolver }
    }
}

impl<'query, D, I, R> TraitSelectionQuery<'query, D, I, R>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
    R: SolverScope<Error = I::Error>,
{
    pub fn with_resolver(context: TyContext<'query, D, I>, resolver: R) -> Self {
        Self { context, resolver }
    }

    /// Give an owned query the same assumptions as inference inside its enclosing item.
    fn with_table<T>(
        &self,
        run: impl for<'s> FnOnce(InferenceTable<'s>, &'s [GenericParamRef]) -> T,
    ) -> Result<T, I::Error> {
        let declarations = SemanticDeclarations::new(&self.context, &self.resolver);
        declarations.with_solver(|solver| {
            let cx = solver.interner();
            let (params, env) = match self.resolver.generic_owner() {
                Some(owner) => (
                    cx.params(owner.into()),
                    cx.parameter_environment(owner.into()),
                ),
                None => (&[][..], Default::default()),
            };
            run(InferenceTable::new(solver, env), params)
        })
    }

    /// Find an impl's generic arguments from an owned receiver, without checking its bounds.
    /// Name lookup uses these arguments to interpret the impl's associated declarations.
    pub(crate) fn match_impl_header(
        &self,
        impl_ref: ImplRef,
        receiver: &Ty,
    ) -> Result<Option<Substitution>, I::Error> {
        let declarations = SemanticDeclarations::new(&self.context, &self.resolver);
        declarations.with_solver(|solver| {
            let cx = solver.interner();
            let params = self
                .resolver
                .generic_owner()
                .map(|owner| cx.params(owner.into()))
                .unwrap_or_default();
            let receiver = cx.lower_ty(receiver, params);
            // A bound such as `where Self::Item: Clone` may have requested this lookup. Use an
            // empty environment so preparing its assumptions cannot request that bound again.
            let table = InferenceTable::new(solver, Default::default());
            table
                .match_impl_header(impl_ref, receiver, None)
                .map(|matched| {
                    matched
                        .subst
                        .finalize(&matched.table, DefId::Impl(impl_ref))
                })
        })
    }

    pub(crate) fn select_impl(
        &self,
        impl_ref: ImplRef,
        receiver: &Ty,
    ) -> Result<Option<SelectedImpl>, I::Error> {
        if matches!(receiver, Ty::Unknown) {
            return Ok(None);
        }
        self.with_table(|table, params| {
            let receiver = table.interner().lower_ty(receiver, params);
            table
                .select_impl(impl_ref, receiver, None)
                .map(SelectedImpl::freeze)
        })
    }

    /// Selection retains distinct source impls even when they produce identical type arguments.
    /// General trait proof in the live solver can also succeed from bounds or builtin rules and
    /// therefore does not use this source-selection API.
    pub fn probe(&self, goal: &TraitGoal) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        let Some(candidates) =
            TraitImplFilter::from(goal.self_ty()).candidates(&self.context, goal.trait_ref())
        else {
            return Ok(ExpectedUnique::Empty);
        };
        self.with_table(|table, params| {
            let cx = table.interner();
            let application = solver::TraitApplication {
                def: goal.trait_ref(),
                args: cx.lower_args(&goal.application.args, params),
            };
            let bindings = goal
                .associated_types
                .iter()
                .map(|bound| cx.lower_assoc_binding(bound, params).clause(cx))
                .collect::<Vec<_>>();
            table
                .select_trait_impl(
                    application,
                    &bindings,
                    candidates.into_iter().map(|candidate| candidate.impl_ref),
                )
                .map(|selected| {
                    let trait_impl = TraitImplRef {
                        impl_ref: selected.impl_ref,
                        trait_ref: application.def,
                    };
                    let result = SelectedImpl::freeze(selected);
                    TraitSelection {
                        trait_impl,
                        application: result.application.expect("trait impl has an application"),
                        subst: result.subst,
                        applicability: result.applicability,
                    }
                })
        })
    }

    /// Resolve associated types where the available bounds and impls provide an answer.
    /// Keep the original type if solving fails or needs unavailable information; callers can
    /// still display a projection such as `T::Item` instead of losing the whole type.
    pub fn normalize_ty(&self, ty: &Ty) -> Result<Ty, I::Error> {
        self.with_table(|table, params| {
            let callbacks = table.interner().track_callbacks();
            let lowered = table.interner().lower_ty(ty, params);
            let normalized = table.normalize(lowered);
            // Goal evaluation cannot detect a failed read which happened while preparing its
            // input. Preserve the original spelling if lowering or setting up the equality failed.
            if callbacks.failure().is_some() {
                return ty.clone();
            }
            match table.fulfill() {
                Outcome::Proven | Outcome::Ambiguous => table.finalize(normalized),
                Outcome::NoSolution | Outcome::Unavailable => ty.clone(),
            }
        })
    }

    /// Resolve a named associated type from a trait goal, including inherited declarations.
    /// Lookup supplies the projection's supertrait arguments; the original trait application
    /// and its equalities are still checked as requirements of this query.
    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        name: &str,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        self.with_table(|table, params| {
            let cx = table.interner();
            let callbacks = cx.track_callbacks();
            let application = solver::TraitApplication {
                def: goal.trait_ref(),
                args: cx.lower_args(&goal.application.args, params),
            };
            // This lookup starts from a complete trait application, so any supertrait syntax
            // belongs to the trait's declaration. Give it a source walk in the existing storage.
            let owner = GenericDefRef::Trait(application.def);
            let Some(context) = self
                .context
                .item_paths()
                .items()
                .type_path_context_for_generic_def(owner)?
            else {
                return Ok(None);
            };
            let mut session = TypeLoweringQuery::new(self.context.item_paths(), &self.resolver)
                .session(
                    cx,
                    TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
                )?;
            let Some(projection) =
                session.associated_type_projection(&application, &Name::new(name))?
            else {
                return Ok(None);
            };
            let bindings = goal
                .associated_types
                .iter()
                .map(|bound| cx.lower_assoc_binding(bound, params).clause(cx))
                .collect::<Vec<_>>();
            if callbacks.failure().is_some() {
                return Ok(None);
            }
            Ok(table
                .normalize_assoc_type(application, &bindings, projection)
                .map(|(ty, outcome)| AssocProjectionResult {
                    ty: table.finalize(ty),
                    applicability: SelectedImpl::applicability(outcome),
                }))
        })?
    }
}
