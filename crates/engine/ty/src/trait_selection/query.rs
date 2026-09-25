//! Owned query results over the same compiler solver used by body inference.
//!
//! Editor queries start from finalized types. Each operation lowers those inputs, proves or
//! normalizes them in a scoped table, and freezes its result before releasing all solver storage.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericParamRef, ImplRef, TraitApplicability, TraitImplRef};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rg_std::ExpectedUnique;
use rustc_type_ir::{self as ir, Upcast as _, inherent::GenericArgs as _};

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
        let mut subst = Substitution::new();
        let table = &selection.table;
        for &param in table.params(DefId::Impl(selection.impl_ref)) {
            if let Some(arg) = selection.subst.get(param) {
                let args = table.finalize_args(solver::List::new(table.interner(), &[arg]));
                subst.push(param, args[0].clone());
            }
        }
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

    fn with_table<T>(
        &self,
        include_bounds: bool,
        run: impl for<'s> FnOnce(InferenceTable<'s>, &'s [GenericParamRef]) -> T,
    ) -> Result<T, I::Error> {
        // Loading a whole function signature here would resolve its return path through this
        // very editor query again. Only generic metadata and declared bounds form the environment.
        let declarations = SemanticDeclarations::new(&self.context, &self.resolver);
        declarations.with_solver(|solver| {
            let cx = solver.interner();
            let paths = self.context.item_paths();
            let (params, clauses, self_trait) = match self.resolver.generic_owner() {
                Some(owner) => {
                    let generics = paths.generics().generics(owner)?;
                    let params = cx.params(DefId::from(owner));
                    let self_trait = generics.iter().find_map(|p| {
                        matches!(p.source(), GenericParamSource::TraitSelf)
                            .then_some(p.param().owner())
                    });
                    let clauses = if include_bounds
                        && let Some(context) =
                            paths.items().type_path_context_for_generic_def(owner)?
                    {
                        TypeLoweringQuery::new(paths, &self.resolver)
                            .session(
                                cx,
                                TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
                            )?
                            .lower_clauses()?
                    } else {
                        Vec::new()
                    };
                    (params, clauses, self_trait)
                }
                None => (&[][..], Vec::new(), None),
            };
            let mut clauses = clauses;
            // A default trait method may use its own trait without a written `Self: Trait`
            // bound. Mirror the body's environment without loading the whole method signature.
            if include_bounds && let Some(owner) = self_trait {
                let owner = DefId::from(owner);
                clauses.push(
                    ir::TraitRef::new_from_args(
                        cx,
                        owner,
                        solver::GenericArgs::identity_for_item(cx, owner),
                    )
                    .upcast(cx),
                );
            }
            let env = solver::ParamEnv(solver::List::new(
                cx,
                &ir::elaborate::elaborate(cx, clauses).collect::<Vec<_>>(),
            ));
            Ok(run(InferenceTable::new(solver, env), params))
        })?
    }

    pub(crate) fn match_impl(
        &self,
        impl_ref: ImplRef,
        receiver: &Ty,
    ) -> Result<Option<SelectedImpl>, I::Error> {
        self.with_table(false, |table, params| {
            let receiver = table.interner().lower_ty(receiver, params);
            table
                .match_impl(impl_ref, receiver, None)
                .map(SelectedImpl::freeze)
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
        self.with_table(true, |table, params| {
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
        self.with_table(true, |table, params| {
            let cx = table.interner();
            let application = solver::TraitApplication {
                def: goal.trait_ref(),
                args: cx.lower_args(&goal.application.args, params),
            };
            let bindings = goal
                .associated_types
                .iter()
                .map(|bound| {
                    application.associated_type_eq(
                        cx,
                        bound.associated_ty,
                        cx.lower_ty(&bound.ty, params),
                    )
                })
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
        self.with_table(true, |table, params| {
            let lowered = table.interner().lower_ty(ty, params);
            let normalized = table.normalize(lowered);
            match table.fulfill() {
                Outcome::Proven | Outcome::Ambiguous => table.finalize(normalized),
                Outcome::NoSolution | Outcome::Unavailable => ty.clone(),
            }
        })
    }

    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        name: &str,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let items = self.context.item_paths().items();
        let Some(alias) = items.declared_associated_type_by_name(goal.trait_ref(), name)? else {
            return Ok(None);
        };
        self.with_table(true, |table, params| {
            let cx = table.interner();
            let application = solver::TraitApplication {
                def: goal.trait_ref(),
                args: cx.lower_args(&goal.application.args, params),
            };
            let bindings = goal
                .associated_types
                .iter()
                .map(|bound| {
                    application.associated_type_eq(
                        cx,
                        bound.associated_ty,
                        cx.lower_ty(&bound.ty, params),
                    )
                })
                .collect::<Vec<_>>();
            table
                .normalize_assoc_type(application, &bindings, alias)
                .map(|(ty, outcome)| AssocProjectionResult {
                    ty: table.finalize(ty),
                    applicability: SelectedImpl::applicability(outcome),
                })
        })
    }
}
