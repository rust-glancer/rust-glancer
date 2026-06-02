//! Lightweight method candidate collection.
//!
//! This module owns method-specific filtering, while impl header matching lives in
//! `impl_match` so receiver-based resolution can share it.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, TraitApplicability};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupIndex, ItemPathQuery, ItemStoreSource};
use rg_ty::NominalTy;

use super::impl_match::BodyImplMatcher;

pub(crate) fn function_applies_to_receiver<'query, D, I>(
    item_paths: ItemPathQuery<'query, D, I>,
    function_ref: FunctionRef,
    receiver_ty: &NominalTy,
) -> Result<bool, PackageStoreError>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = PackageStoreError>,
{
    BodyImplMatcher::new(item_paths)
        .semantic_function_applies_to_receiver(function_ref, receiver_ty)
}

pub(crate) fn trait_function_candidates_for_receiver<'query, D, I>(
    index: Option<&ItemLookupIndex>,
    item_paths: ItemPathQuery<'query, D, I>,
    receiver_ty: &NominalTy,
    method_name: Option<&str>,
) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Clone,
{
    let matcher = BodyImplMatcher::new(item_paths.clone());
    let item_query = item_paths.items();
    let mut functions = Vec::new();
    let trait_impls = match index {
        Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
        None => item_query.trait_impls_for_type(receiver_ty.def)?,
    };

    for trait_impl in trait_impls {
        // For method calls, the name is known before we do any trait-impl compatibility work.
        // If the indexed trait has no function with that name, this impl cannot contribute a
        // candidate regardless of how well the impl header matches the receiver.
        let mut indexed_trait_functions = None;
        if let (Some(index), Some(method_name)) = (index, method_name)
            && let Some(functions) =
                index.trait_functions_by_name(trait_impl.trait_ref, method_name)
        {
            if functions.is_empty() {
                continue;
            }
            indexed_trait_functions = Some(functions.to_vec());
        }

        let Some(trait_impl_match) = matcher.semantic_trait_impl_match(trait_impl, receiver_ty)?
        else {
            continue;
        };
        let (applicability, _) = trait_impl_match.into_parts();

        let trait_functions = if let Some(functions) = indexed_trait_functions {
            functions
        } else {
            let trait_functions = if let Some(index) = index
                && let Some(functions) = index.trait_functions(trait_impl.trait_ref)
            {
                functions.to_vec()
            } else {
                item_query
                    .trait_data(trait_impl.trait_ref)?
                    .map(|t| t.functions().collect())
                    .unwrap_or_default()
            };

            // The direct Semantic IR fallback cannot skip the impl check up front, but it can
            // still avoid returning unrelated trait functions to the later method-call filter.
            if let Some(method_name) = method_name {
                let mut retained = Vec::new();
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
            push_function_candidate(&mut functions, function, applicability);
        }
    }

    Ok(functions)
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
        *existing = best_applicability(*existing, applicability);
        return;
    }

    functions.push((function, applicability));
}

fn best_applicability(left: TraitApplicability, right: TraitApplicability) -> TraitApplicability {
    match (left, right) {
        (TraitApplicability::Yes, _) | (_, TraitApplicability::Yes) => TraitApplicability::Yes,
        (TraitApplicability::Maybe, _) | (_, TraitApplicability::Maybe) => {
            TraitApplicability::Maybe
        }
        (TraitApplicability::No, TraitApplicability::No) => TraitApplicability::No,
    }
}
