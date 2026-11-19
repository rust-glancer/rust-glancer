//! Discover function declarations and check them against one live receiver.
//!
//! Named calls and completion use the same declarations, lexical overlays, and candidate trials.
//! Dot calls and completion keep both inherent and trait declarations. Dot calls compare their
//! self parameters with the receiver adjustments before applying inherent precedence. Associated
//! calls already fix Self in the path and can apply that precedence here.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, ImplRef, ItemOwner, ScopeId, TraitApplicability, TraitDefRef};
use rg_semantic_ir::{FunctionData, SemanticItemSource};
use rg_source::ReadError;
use rg_std::UniqueVec;
use rg_ty::{
    lookup::MemberMethodCandidateRef,
    solver::{
        DefId, ImplSelection, InferenceSubstitution, InferenceTable, Outcome, TraitApplication, Ty,
        TyShape,
    },
};

use super::LiveBodyQuery;
use crate::resolution::cache::BodyTraitSurface;

/// The syntax being resolved determines name filtering and whether a self receiver is required.
/// Dot calls need both origins to compare receiver adjustments before choosing a method.
/// Associated calls use the path's Self type, without receiver adjustments.
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

impl<'s> MemberCandidate<'s> {
    fn inherent(function: FunctionRef, selection: ImplSelection<'s>) -> Self {
        Self {
            function,
            trait_ref: None,
            subst: selection.subst,
            table: selection.table,
            outcome: selection.outcome,
        }
    }

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
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Find candidate functions for one possible `Self` type at the call site. For example,
    /// resolving `v.pop()` on a `Vec<T>` can ask first about `Vec<T>` and then about `[T]`
    /// after dereferencing. Each result carries what inference learned while checking that
    /// candidate, so the caller can keep those assignments if it selects the function.
    pub(crate) fn function_candidates<'s>(
        &self,
        scope: ScopeId,
        receiver: Ty<'s>,
        request: FunctionLookup<'_, 's>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<MemberCandidate<'s>>, ReadError> {
        let (name, qualification) = match request {
            FunctionLookup::Method(name) => (Some(name), None),
            FunctionLookup::Associated {
                name,
                qualification,
            } => (Some(name), qualification),
            FunctionLookup::Completion => (None, None),
        };
        let needs_self = !matches!(request, FunctionLookup::Associated { .. });
        let accepts = |data: &FunctionData| {
            name.is_none_or(|name| data.name == name) && (!needs_self || data.has_self_receiver())
        };
        let body_items = self.context.body_local_items();
        let items = self.context.item_query();
        let lookup = self.context.item_lookup_query();
        let mut candidates = Vec::new();

        if qualification.is_none() {
            if let Some(adt) = receiver.as_adt() {
                // Look up crate functions by the type's definition and add impls declared
                // inside the body. The index groups `Vec<u8>` and `Vec<u16>` under the same `Vec`
                // definition, so each function's impl still needs to match the receiver.
                let mut functions = UniqueVec::new();
                for &impl_ref in body_items.inherent_impls_for_type(adt.def)? {
                    functions.extend(
                        items
                            .associated_functions(impl_ref.member_store())?
                            .map(|(function, _)| function),
                    );
                }
                match name {
                    Some(name) => functions
                        .extend(lookup.inherent_functions_for_type_and_name(adt.def, name)?),
                    None => functions.extend(lookup.inherent_functions_for_type(&items, adt.def)?),
                }
                for function in functions {
                    let Some(data) = items.function_data(function)? else {
                        continue;
                    };
                    let ItemOwner::Impl(id) = data.owner else {
                        continue;
                    };
                    if accepts(data)
                        && let Some(selection) = table.select_impl(
                            ImplRef::new(function.store.origin, id),
                            receiver,
                            None,
                        )
                    {
                        candidates.push(MemberCandidate::inherent(function, selection));
                    }
                }
            } else if !matches!(
                receiver.shape(),
                TyShape::InferVar { .. } | TyShape::Unknown | TyShape::Param(_) | TyShape::Alias(_)
            ) {
                // A lookup on `[T]` can turn up `impl u8 { methods!(); }` because the macro
                // might declare the requested name. Checking the receiver first rules out
                // that impl without expanding its methods.
                for impl_ref in lookup.structural_impls_for_function(name)? {
                    let Some(selection) = table.select_impl(impl_ref, receiver, None) else {
                        continue;
                    };
                    let mut functions = items
                        .associated_functions(impl_ref.member_store())?
                        .filter_map(|(function, data)| accepts(data).then_some(function));
                    let Some(mut function) = functions.next() else {
                        continue;
                    };
                    // Each candidate needs its own inference table. Let the last match take
                    // the original table, so finding just one method requires no copy.
                    for next in functions {
                        candidates.push(MemberCandidate::inherent(function, selection.clone()));
                        function = next;
                    }
                    candidates.push(MemberCandidate::inherent(function, selection));
                }
            }
            if matches!(request, FunctionLookup::Associated { .. }) && !candidates.is_empty() {
                return Ok(candidates);
            }
        }

        // A trait declaration is enough to discover a method. Applicability can come from a
        // caller bound, a trait object, or an opaque type, even when there are no concrete impls at all.
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
            }?;
            let functions = match indexed {
                Some(functions) => functions,
                None => items
                    .associated_functions(trait_ref.member_store())?
                    .map(|(function, _)| function)
                    .collect(),
            };
            for function in functions {
                let Some(data) = items.function_data(function)? else {
                    continue;
                };
                if !accepts(data) {
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
