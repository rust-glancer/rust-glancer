//! Discover function declarations and check them against one live receiver.
//!
//! Named calls and completion use the same declarations, lexical overlays, and candidate trials.
//! A named call gives inherent declarations precedence. Completion keeps both origins, and its
//! caller visits every receiver adjustment instead of stopping at the first match.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, ImplRef, ItemOwner, ScopeId, TraitApplicability, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::{OperationError, UniqueVec};
use rg_ty::{
    lookup::MemberMethodCandidateRef,
    solver::{
        DefId, InferenceSubstitution, InferenceTable, Outcome, TraitApplication, Ty, TyShape,
    },
};

use super::LiveBodyQuery;
use crate::resolution::cache::BodyTraitSurface;

/// The syntax being resolved determines name filtering and whether a self receiver is required.
/// Completion deliberately collects across inherent and trait origins; named calls apply inherent
/// precedence before returning candidates to inference.
#[derive(Clone, Copy)]
pub(crate) enum FunctionLookup<'a, 's> {
    Method(&'a str),
    Associated {
        name: &'a str,
        qualification: Option<TraitApplication<'s>>,
    },
    Completion,
}

/// Evidence for one declaration at one receiver adjustment. Each candidate owns its trial so
/// another declaration cannot inherit its assignments. Calls may adopt it; completion exports
/// only the declaration and applicability, then drops the trial.
pub(crate) struct MemberCandidate<'s> {
    pub function: FunctionRef,
    pub trait_ref: Option<TraitDefRef>,
    pub subst: InferenceSubstitution<'s>,
    pub table: InferenceTable<'s>,
    pub outcome: Outcome,
}

impl MemberCandidate<'_> {
    pub(crate) fn method_ref(&self) -> MemberMethodCandidateRef {
        match self.trait_ref {
            Some(_) => MemberMethodCandidateRef::trait_method(
                self.function,
                match self.outcome {
                    Outcome::Proven => TraitApplicability::Yes,
                    Outcome::NoSolution => TraitApplicability::No,
                    Outcome::Ambiguous | Outcome::Unavailable => TraitApplicability::Maybe,
                },
            ),
            None => MemberMethodCandidateRef::inherent(self.function),
        }
    }
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn function_candidates<'s>(
        &self,
        scope: ScopeId,
        receiver: Ty<'s>,
        request: FunctionLookup<'_, 's>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<MemberCandidate<'s>>, PackageStoreError> {
        let (name, qualification) = match request {
            FunctionLookup::Method(name) => (Some(name), None),
            FunctionLookup::Associated {
                name,
                qualification,
            } => (Some(name), qualification),
            FunctionLookup::Completion => (None, None),
        };
        let needs_self = !matches!(request, FunctionLookup::Associated { .. });
        let body_items = self.context.body_local_items();
        let items = self.context.item_query();
        let lookup = self.context.item_lookup_query();
        let mut candidates = Vec::new();

        // Body-local declarations replace saved declarations of the same name. A named call
        // reads the name index; only completion expands the whole inherent declaration surface.
        if qualification.is_none() {
            let mut functions = UniqueVec::new();
            let local_names = if let Some(adt) = receiver.as_adt() {
                for &impl_ref in body_items.inherent_impls_for_type(adt.def)? {
                    if let Some(data) = items.impl_data(impl_ref)? {
                        functions.extend(data.functions());
                    }
                }
                let names = body_items.inherent_item_names_for_type(adt.def)?;
                match name {
                    Some(name) if names.is_some_and(|names| names.contains_function(name)) => {}
                    Some(name) => {
                        if let Ok(saved) =
                            lookup.inherent_functions_for_type_and_name(adt.def, name)
                        {
                            functions.extend(saved);
                        }
                    }
                    None => match lookup.inherent_functions_for_type(&items, adt.def) {
                        Ok(saved) => functions.extend(saved),
                        Err(OperationError::Source(error)) => return Err(error),
                        Err(OperationError::Cancelled(_)) => return Ok(Vec::new()),
                    },
                }
                names
            } else {
                if !matches!(
                    receiver.shape(),
                    TyShape::InferVar { .. }
                        | TyShape::Unknown
                        | TyShape::Param(_)
                        | TyShape::Alias(_)
                ) {
                    match name {
                        Some(name) => {
                            if let Ok(saved) = lookup.structural_inherent_functions_by_name(name) {
                                functions = saved;
                            }
                        }
                        None => {
                            let Ok(impls) = lookup.structural_inherent_impls() else {
                                return Ok(Vec::new());
                            };
                            for impl_ref in impls {
                                if let Some(data) = items.impl_data(impl_ref)? {
                                    functions.extend(data.functions());
                                }
                            }
                        }
                    }
                }
                None
            };
            for function in functions {
                let Some(data) = items.function_data(function)? else {
                    continue;
                };
                if name.is_some_and(|name| data.name != name)
                    || needs_self && !data.has_self_receiver()
                    || function.origin.as_crate_ref().is_some()
                        && local_names
                            .is_some_and(|names| names.contains_function(data.name.as_str()))
                {
                    continue;
                }
                let ItemOwner::Impl(id) = data.owner else {
                    continue;
                };
                let impl_ref = ImplRef {
                    origin: function.origin,
                    id,
                };
                let Some(selection) = table.select_impl(impl_ref, receiver, None) else {
                    continue;
                };
                candidates.push(MemberCandidate {
                    function,
                    trait_ref: None,
                    subst: selection.subst,
                    table: selection.table,
                    outcome: selection.outcome,
                });
            }
            if !matches!(request, FunctionLookup::Completion) && !candidates.is_empty() {
                return Ok(candidates);
            }
        }

        // A trait declaration is enough to discover a method. Applicability can come from a
        // caller bound or an opaque type's bounds, even when there are no concrete impls at all.
        let traits = match qualification {
            Some(application) => [application.def].into_iter().collect(),
            None => {
                let surface =
                    name.map_or(BodyTraitSurface::Functions, BodyTraitSurface::FunctionNamed);
                let mut traits = (*self.context.traits().refs_for_surface(scope, surface)?).clone();
                traits.extend(table.bound_traits(receiver));
                traits
            }
        };
        for trait_ref in traits {
            let indexed = match name {
                Some(name) => lookup.trait_functions_by_name(trait_ref, name),
                None => lookup.trait_functions(trait_ref),
            };
            let Ok(indexed) = indexed else {
                return Ok(Vec::new());
            };
            let functions = match indexed {
                Some(functions) => functions,
                None => {
                    let Some(data) = items.trait_data(trait_ref)? else {
                        continue;
                    };
                    data.functions().collect()
                }
            };
            for function in functions {
                let Some(data) = items.function_data(function)? else {
                    continue;
                };
                if name.is_some_and(|name| data.name != name)
                    || needs_self && !data.has_self_receiver()
                {
                    continue;
                }
                let trial = table.probe();
                let cx = trial.interner();
                // Parameter metadata is part of checking this candidate. Missing slots must
                // not become a different trait application, and cannot affect the next trial.
                let callbacks = cx.track_callbacks();
                let owner = DefId::Trait(trait_ref);
                let mut subst = qualification.map_or_else(
                    || trial.fresh_substitution(owner),
                    |application| {
                        InferenceSubstitution::from_args(
                            trial.params(owner).iter().copied(),
                            application.args,
                        )
                    },
                );
                if let Some(self_param) = trial.params(owner).first() {
                    subst.insert(*self_param, receiver.into());
                }
                let args = subst.args_for(cx, trial.params(owner).iter().copied());
                if callbacks.failure().is_some() {
                    continue;
                }
                let outcome = trial.prove([TraitApplication {
                    def: trait_ref,
                    args,
                }
                .clause(cx)]);
                if outcome == Outcome::NoSolution {
                    continue;
                }
                candidates.push(MemberCandidate {
                    function,
                    trait_ref: Some(trait_ref),
                    subst,
                    table: trial,
                    outcome,
                });
            }
        }
        Ok(candidates)
    }
}
