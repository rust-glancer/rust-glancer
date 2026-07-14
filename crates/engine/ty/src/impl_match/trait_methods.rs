//! Best-effort trait impl matching for method candidates.
//!
//! Editor-facing method lookup can preserve useful `Maybe` candidates when a proof would require
//! deeper solving. Simple direct cases still reuse bounded trait selection for consistency.

use crate::{
    AdtTy, Substitution, TraitSelectionOptions, TraitSelectionQuery, Ty, TypePathResolver,
};
use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, TraitApplicability, TraitImplRef};
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_std::UniqueVec;

use super::ImplMatcher;

/// Result of matching one trait impl header against a receiver type.
struct TraitImplMatch {
    applicability: TraitApplicability,
    subst: Substitution,
}

impl TraitImplMatch {
    /// Creates a match result from the computed confidence and receiver substitutions.
    fn new(applicability: TraitApplicability, subst: Substitution) -> Self {
        Self {
            applicability,
            subst,
        }
    }

    /// Confidence that the impl header applies to the receiver.
    fn applicability(&self) -> TraitApplicability {
        self.applicability
    }

    /// Splits the result into the match confidence and substitutions for associated signatures.
    fn into_parts(self) -> (TraitApplicability, Substitution) {
        (self.applicability, self.subst)
    }
}

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Returns only the yes/maybe/no part of `trait_impl_match`.
    pub fn trait_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
    ) -> Result<TraitApplicability, D::Error> {
        Ok(self
            .trait_impl_match(
                trait_impl,
                receiver_ty,
                TraitSelectionOptions::new().header_only(),
            )?
            .map(|trait_impl_match| trait_impl_match.applicability())
            .unwrap_or(TraitApplicability::No))
    }

    /// Returns trait-associated function candidates for a nominal receiver.
    pub fn trait_function_candidates_for_receiver(
        &self,
        index: &ItemLookupIndex,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let trait_impls = index
            .trait_impls_for_type(receiver_ty.def)
            .cloned()
            .unwrap_or_default();
        self.trait_function_candidates_from_impls(index, trait_impls, receiver_ty, method_name)
    }

    /// Returns named trait methods while leaving impl predicates for body inference to validate.
    pub fn trait_function_candidates_for_receiver_with_options(
        &self,
        index: &ItemLookupIndex,
        receiver_ty: &AdtTy,
        method_name: &str,
        options: TraitSelectionOptions,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let trait_impls = index
            .trait_impls_for_type(receiver_ty.def)
            .cloned()
            .unwrap_or_default();
        self.trait_function_candidates_from_impls_with_options(
            index,
            trait_impls,
            receiver_ty,
            Some(method_name),
            options,
        )
    }

    /// Expands already-collected trait impl candidates into trait function candidates.
    ///
    /// The caller owns visibility and overlay rules by deciding which trait impl refs to pass in;
    /// this method owns only receiver applicability and trait-associated function expansion.
    pub fn trait_function_candidates_from_impls(
        &self,
        index: &ItemLookupIndex,
        trait_impls: UniqueVec<TraitImplRef>,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        self.trait_function_candidates_from_impls_with_options(
            index,
            trait_impls,
            receiver_ty,
            method_name,
            TraitSelectionOptions::new().header_only(),
        )
    }

    /// Expands trait impls under an explicit predicate-ownership policy.
    pub fn trait_function_candidates_from_impls_with_options(
        &self,
        index: &ItemLookupIndex,
        trait_impls: UniqueVec<TraitImplRef>,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
        options: TraitSelectionOptions,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let item_query = self.item_paths.items();
        let mut functions = Vec::new();
        for trait_impl in trait_impls {
            // For method calls, the name is known before we do any trait-impl compatibility work.
            // If the indexed trait has no function with that name, this impl cannot contribute a
            // candidate regardless of how well the impl header matches the receiver.
            let mut indexed_trait_functions = None;
            if let Some(method_name) = method_name
                && let Some(indexed_functions) =
                    index.trait_functions_by_name(trait_impl.trait_ref, method_name)
            {
                if indexed_functions.is_empty() {
                    continue;
                }
                indexed_trait_functions = indexed_functions.functions().cloned();
            }

            let Some(trait_impl_match) = self.trait_impl_match(trait_impl, receiver_ty, options)?
            else {
                continue;
            };
            let (applicability, _) = trait_impl_match.into_parts();

            let trait_functions = if let Some(functions) = indexed_trait_functions {
                functions
            } else if let Some(functions) = index.trait_functions(trait_impl.trait_ref) {
                functions.clone()
            } else {
                let trait_functions = item_query
                    .trait_data(trait_impl.trait_ref)?
                    .map(|t| t.functions().collect())
                    .unwrap_or_default();

                // The direct item-store fallback cannot skip the impl check up front, but it can
                // still avoid returning unrelated trait functions to the later method-call filter.
                if let Some(method_name) = method_name {
                    let mut retained = UniqueVec::new();
                    for function in trait_functions {
                        let Some(function_data) = item_query.function_data(function)? else {
                            continue;
                        };
                        if function_data.name == method_name {
                            retained.push(function);
                        }
                    }
                    retained
                } else {
                    trait_functions
                }
            };

            for function in trait_functions {
                Self::push_function_candidate(&mut functions, function, applicability);
            }
        }

        Ok(functions)
    }

    /// Matches one trait impl against a receiver.
    ///
    /// For `impl<T> Trait for Wrapper<T>` and receiver `Wrapper<User>`, this returns an
    /// `TraitImplMatch` whose substitutions include `T -> User`.
    fn trait_impl_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        options: TraitSelectionOptions,
    ) -> Result<Option<TraitImplMatch>, D::Error> {
        let item_query = self.item_paths.items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_ty.is(&receiver_ty.def)
            || !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
            || !options.accepts_impl_header(impl_data)
        {
            return Ok(None);
        }

        let Some(header) = self.impl_header(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        let Some((subst, self_applicability)) =
            Self::impl_self_subst(&header, &Ty::adt(receiver_ty.clone()))
        else {
            return Ok(None);
        };
        let mut applicability = self_applicability;

        // Receiver matching above already used this exact canonical header. Method lookup either
        // rejects predicates or deliberately leaves them to Body IR, so submitting a goal rebuilt
        // from the same impl would only match the candidate against itself a second time. Keep the
        // full selection path for strict callers that actually ask Chalk to prove the predicates.
        if options.should_solve_impl_predicates() {
            let Some(mut trait_ref) = header.trait_ref.clone() else {
                return Ok(None);
            };
            trait_ref.application.args = trait_ref
                .application
                .args
                .iter()
                .map(|arg| subst.apply_arg(arg))
                .collect();
            trait_ref.associated_types = trait_ref
                .associated_types
                .into_iter()
                .map(|binding| crate::AssocTypeBinding {
                    associated_ty: binding.associated_ty,
                    ty: subst.apply(&binding.ty),
                })
                .collect();
            let goal = crate::TraitGoal::from_lowering(trait_ref);
            let table = crate::inference::InferenceTable::new();
            let Some(selection) = TraitSelectionQuery::probe_visible_trait_impl(
                &self.item_paths,
                &self.crate_items,
                &goal,
                &table,
                trait_impl,
                &header,
                options,
                &self.trait_selection_cache,
            )?
            else {
                return Ok(None);
            };
            applicability = applicability.and(selection.applicability);
        }

        if options.leaves_impl_predicates_to_caller() && !header.clauses.is_empty() {
            applicability = applicability.and(TraitApplicability::Maybe);
        }
        Ok(Some(TraitImplMatch::new(applicability, subst)))
    }

    fn push_function_candidate(
        functions: &mut Vec<(FunctionRef, TraitApplicability)>,
        function: FunctionRef,
        applicability: TraitApplicability,
    ) {
        if let Some((_, existing)) = functions
            .iter_mut()
            .find(|(existing_function, _)| *existing_function == function)
        {
            *existing = existing.or(applicability);
            return;
        }

        functions.push((function, applicability));
    }
}
