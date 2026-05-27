//! Lightweight method candidate collection.
//!
//! This module owns method-specific filtering, while impl header matching lives in
//! `impl_match` so receiver-based resolution can share it.

use rg_def_map::DefMapReadTxn;
use rg_ir_model::{FunctionRef, TraitApplicability};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_ty::{IndexedLocalNominalTy, IndexedNominalTy};

use crate::{
    ir::body::BodyData,
    ir::ids::{BodyFunctionRef, BodyRef},
};

use super::{
    SemanticResolutionIndex,
    impl_match::{BodyImplMatcher, LocalImplMatcher},
};

pub(super) fn semantic_function_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: FunctionRef,
    receiver_ty: &IndexedNominalTy,
) -> Result<bool, PackageStoreError> {
    BodyImplMatcher::new(def_map, semantic_ir)
        .semantic_function_applies_to_receiver(function_ref, receiver_ty)
}

pub(super) fn semantic_trait_function_candidates_for_receiver(
    index: Option<&SemanticResolutionIndex>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    receiver_ty: &IndexedNominalTy,
    method_name: Option<&str>,
) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
    let matcher = BodyImplMatcher::new(def_map, semantic_ir);
    let mut functions = Vec::new();
    let trait_impls = match index {
        Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
        None => semantic_ir.trait_impls_for_type(receiver_ty.def)?,
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
            let trait_functions = match index {
                Some(index) => match index.trait_functions(trait_impl.trait_ref) {
                    Some(functions) => functions.to_vec(),
                    None => semantic_ir.trait_functions(trait_impl.trait_ref)?,
                },
                None => semantic_ir.trait_functions(trait_impl.trait_ref)?,
            };

            // The direct Semantic IR fallback cannot skip the impl check up front, but it can
            // still avoid returning unrelated trait functions to the later method-call filter.
            if let Some(method_name) = method_name {
                let mut retained = Vec::new();
                for function in trait_functions {
                    let Some(function_data) = semantic_ir.function_data(function)? else {
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

pub(super) fn local_function_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    body_ref: BodyRef,
    body: &BodyData,
    function_ref: BodyFunctionRef,
    receiver_ty: &IndexedLocalNominalTy,
) -> Result<bool, PackageStoreError> {
    LocalImplMatcher::new(def_map, semantic_ir, body_ref, body)
        .local_function_applies_to_receiver(function_ref, receiver_ty)
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
