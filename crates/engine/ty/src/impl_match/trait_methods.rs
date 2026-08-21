//! Best-effort trait impl matching for method candidates.
//!
//! Header compatibility goes through native candidate discovery, then the shared selection query
//! proves the exact candidate's predicates. A definite rejection removes the method; ambiguity or
//! unsupported body-local evidence remains a useful editor-facing `Maybe` candidate.

use crate::{
    AdtTy, TraitSelection, TraitSelectionQuery, Ty, TypePathResolver, inference::InferenceTable,
};
use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, FunctionRef, TraitApplicability, TraitImplRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use super::ImplMatcher;

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Returns only the yes/maybe/no part of `trait_impl_match`.
    pub fn trait_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        table: &InferenceTable,
    ) -> Result<TraitApplicability, D::Error> {
        Ok(self
            .trait_impl_match(trait_impl, receiver_ty, table)?
            .map(|selection| selection.applicability)
            .unwrap_or(TraitApplicability::No))
    }

    /// Returns trait-associated function candidates for a nominal receiver.
    pub fn trait_function_candidates_for_receiver(
        &self,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
        table: &InferenceTable,
    ) -> Result<Vec<(FunctionRef, TraitSelection)>, D::Error> {
        let trait_impls = self
            .context
            .item_lookup()
            .trait_impls_for_type(receiver_ty.def);
        self.trait_function_candidates_from_impls(trait_impls, receiver_ty, method_name, table)
    }

    /// Expands already-collected trait impl candidates into trait function candidates.
    ///
    /// The caller owns visibility and overlay rules by deciding which trait impl refs to pass in;
    /// this method owns only receiver applicability and trait-associated function expansion.
    pub fn trait_function_candidates_from_impls(
        &self,
        trait_impls: UniqueVec<TraitImplRef>,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
        table: &InferenceTable,
    ) -> Result<Vec<(FunctionRef, TraitSelection)>, D::Error> {
        let item_query = self.context.item_paths().items();
        let mut functions = Vec::new();
        for trait_impl in trait_impls {
            // For method calls, the name is known before we do any trait-impl compatibility work.
            // If the indexed trait has no function with that name, this impl cannot contribute a
            // candidate regardless of how well the impl header matches the receiver.
            let mut indexed_trait_functions = None;
            if let Some(method_name) = method_name
                && let Some(indexed_functions) = self
                    .context
                    .item_lookup()
                    .trait_functions_by_name(trait_impl.trait_ref, method_name)
            {
                if indexed_functions.is_empty() {
                    continue;
                }
                indexed_trait_functions = Some(indexed_functions);
            }

            let Some(selection) = self.trait_impl_match(trait_impl, receiver_ty, table)? else {
                continue;
            };

            let trait_functions = if let Some(functions) = indexed_trait_functions {
                functions
            } else if let Some(functions) = self
                .context
                .item_lookup()
                .trait_functions(trait_impl.trait_ref)
            {
                functions
            } else {
                // Crate traits are guaranteed to be present in the semantic lookup query. A
                // body-local trait deliberately is not: its declaration lives in the active body
                // store, alongside the local impl passed in by the caller.
                if !matches!(trait_impl.trait_ref.origin, DefMapRef::Body(_)) {
                    continue;
                }
                let trait_functions = item_query
                    .trait_data(trait_impl.trait_ref)?
                    .map(|t| t.functions().collect())
                    .unwrap_or_default();

                // Body-local traits have no name index, so filter their functions after the impl
                // applicability check instead.
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
                // Keep the selected impl with the function. Two overlapping impls can expose the
                // same trait declaration but carry different predicate evidence; collapsing them
                // by function identity would make an ambiguous call look uniquely selected.
                functions.push((function, selection.clone()));
            }
        }

        Ok(functions)
    }

    /// Matches one trait impl against a receiver.
    ///
    /// For `impl<T> Trait for Wrapper<T>` and receiver `Wrapper<User>`, header matching first binds
    /// `T -> User`; Chalk then classifies the instantiated predicates before the applicability is
    /// returned.
    fn trait_impl_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        table: &InferenceTable,
    ) -> Result<Option<TraitSelection>, D::Error> {
        let item_query = self.context.item_paths().items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_ty.is(&receiver_ty.def)
            || !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
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
        let Some(mut selection) = TraitSelectionQuery::new(self.context.clone())
            .probe_instantiated_impl(trait_impl, &header, subst, table)?
        else {
            return Ok(None);
        };

        selection.applicability = self_applicability.and(selection.applicability);
        Ok(selection.applicability.is_applicable().then_some(selection))
    }
}
