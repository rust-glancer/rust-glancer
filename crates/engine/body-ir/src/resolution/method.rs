//! Lightweight semantic method matching.
//!
//! This module checks whether an impl method is a plausible candidate for a known receiver type.
//! It is intentionally not a trait solver: it only compares explicit nominal self types and args.

use rg_def_map::DefMapReadTxn;
use rg_item_tree::{GenericParams, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{
    FunctionRef, ImplRef, ItemOwner, SemanticIrReadTxn, TraitApplicability, TraitImplRef,
    TypePathContext,
};

use crate::{
    body::BodyData,
    ids::{BodyFunctionRef, BodyRef},
    item::{BodyFunctionOwner, BodyImplData},
    ty::{BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

use super::{
    SemanticResolutionIndex,
    ty::{
        TypeSubst, body_generic_arg_ty, generic_arg_type_ref, ty_from_type_ref_in_context,
        type_param_name_from_type_ref,
    },
    type_path::BodyTypePathResolver,
};

pub(super) fn semantic_function_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: FunctionRef,
    receiver_ty: &BodyNominalTy,
) -> Result<bool, PackageStoreError> {
    // Trait items are shared by all impl candidates in the current best-effort model. Inherent
    // impl items, however, must at least match the receiver's resolved self type.
    let Some(function_data) = semantic_ir.function_data(function_ref)? else {
        return Ok(false);
    };
    let ItemOwner::Impl(impl_id) = function_data.owner else {
        return Ok(true);
    };
    let impl_ref = ImplRef {
        target: function_ref.target,
        id: impl_id,
    };
    let Some(impl_data) = semantic_ir.impl_data(impl_ref)? else {
        return Ok(false);
    };
    if !impl_data.resolved_self_tys.contains(&receiver_ty.def) {
        return Ok(false);
    }

    impl_self_args_match_receiver(def_map, semantic_ir, impl_ref, impl_data, receiver_ty)
}

pub(super) fn semantic_impl_self_subst(
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: FunctionRef,
    receiver_ty: &BodyNominalTy,
) -> TypeSubst {
    // Convert the impl header into substitutions for method signatures. For
    // `impl<U> Wrapper<U>`, a `Wrapper<User>` receiver gives `U -> User`.
    let Ok(Some(function_data)) = semantic_ir.function_data(function_ref) else {
        return TypeSubst::new();
    };
    let ItemOwner::Impl(impl_id) = function_data.owner else {
        return TypeSubst::new();
    };
    let Ok(Some(impl_data)) = semantic_ir.impl_data(ImplRef {
        target: function_ref.target,
        id: impl_id,
    }) else {
        return TypeSubst::new();
    };
    let TypeRef::Path(self_ty) = &impl_data.self_ty else {
        return TypeSubst::new();
    };
    let Some(segment) = self_ty.segments.last() else {
        return TypeSubst::new();
    };

    let impl_type_params = impl_type_param_names(&impl_data.generics);
    let receiver_type_args = receiver_ty
        .args
        .iter()
        .filter_map(body_generic_arg_ty)
        .collect::<Vec<_>>();

    segment
        .args
        .iter()
        .filter_map(generic_arg_type_ref)
        .zip(receiver_type_args)
        .filter_map(|(impl_arg, receiver_arg)| {
            let name = type_param_name_from_type_ref(impl_arg)?;
            impl_type_params
                .contains(&name.as_str())
                .then_some((name, receiver_arg))
        })
        .collect()
}

pub(super) fn semantic_trait_function_candidates_for_receiver(
    index: Option<&SemanticResolutionIndex>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    receiver_ty: &BodyNominalTy,
    method_name: Option<&str>,
) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
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

        let applicability =
            semantic_trait_impl_applicability(def_map, semantic_ir, trait_impl, receiver_ty)?;
        if !applicability.is_applicable() {
            continue;
        }

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
    receiver_ty: &BodyLocalNominalTy,
) -> Result<bool, PackageStoreError> {
    // Body-local inherent impls are selected by exact local item identity, then refined by the
    // same shallow generic-argument compatibility rule used for module-level impls.
    if function_ref.body != receiver_ty.item.body {
        return Ok(false);
    }
    let Some(function_data) = body.local_function(function_ref.function) else {
        return Ok(false);
    };
    let BodyFunctionOwner::LocalImpl(impl_id) = function_data.owner;
    let Some(impl_data) = body.local_impl(impl_id) else {
        return Ok(false);
    };
    if impl_data.self_item != Some(receiver_ty.item) || impl_data.trait_ref.is_some() {
        // Body-local trait impls are an explicit non-goal for now. They are rare enough that
        // modeling their lookup would add more complexity than useful LSP signal at this stage.
        return Ok(false);
    }

    local_impl_self_args_match_receiver(
        def_map,
        semantic_ir,
        body_ref,
        body,
        impl_data,
        receiver_ty,
    )
}

pub(super) fn local_impl_self_subst(
    body: &BodyData,
    function_ref: BodyFunctionRef,
    receiver_ty: &BodyLocalNominalTy,
) -> TypeSubst {
    // Convert body-local impl generics into method-signature substitutions. For
    // `impl<U> Wrapper<U>`, a `Wrapper<User>` receiver gives `U -> User`.
    if function_ref.body != receiver_ty.item.body {
        return TypeSubst::new();
    }
    let Some(function_data) = body.local_function(function_ref.function) else {
        return TypeSubst::new();
    };
    let BodyFunctionOwner::LocalImpl(impl_id) = function_data.owner;
    let Some(impl_data) = body.local_impl(impl_id) else {
        return TypeSubst::new();
    };
    let TypeRef::Path(self_ty) = &impl_data.self_ty else {
        return TypeSubst::new();
    };
    let Some(segment) = self_ty.segments.last() else {
        return TypeSubst::new();
    };

    let impl_type_params = impl_type_param_names(&impl_data.generics);
    let receiver_type_args = receiver_ty
        .args
        .iter()
        .filter_map(body_generic_arg_ty)
        .collect::<Vec<_>>();

    segment
        .args
        .iter()
        .filter_map(generic_arg_type_ref)
        .zip(receiver_type_args)
        .filter_map(|(impl_arg, receiver_arg)| {
            let name = type_param_name_from_type_ref(impl_arg)?;
            impl_type_params
                .contains(&name.as_str())
                .then_some((name, receiver_arg))
        })
        .collect()
}

fn impl_self_args_match_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    impl_ref: ImplRef,
    impl_data: &rg_semantic_ir::ImplData,
    receiver_ty: &BodyNominalTy,
) -> Result<bool, PackageStoreError> {
    // This is a shallow compatibility check. Impl type parameters behave as wildcards, while
    // concrete args such as `impl Wrapper<User>` must equal the receiver's known args.
    let TypeRef::Path(self_ty) = &impl_data.self_ty else {
        return Ok(true);
    };
    let Some(segment) = self_ty.segments.last() else {
        return Ok(true);
    };

    let impl_type_args = segment
        .args
        .iter()
        .filter_map(generic_arg_type_ref)
        .collect::<Vec<_>>();
    if impl_type_args.is_empty() {
        return Ok(true);
    }

    let receiver_type_args = receiver_ty
        .args
        .iter()
        .filter_map(body_generic_arg_ty)
        .collect::<Vec<_>>();
    if impl_type_args.len() != receiver_type_args.len() {
        return Ok(false);
    }

    let impl_type_params = impl_type_param_names(&impl_data.generics);
    for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
        if type_param_name_from_type_ref(impl_arg)
            .as_deref()
            .is_some_and(|name| impl_type_params.contains(&name))
        {
            continue;
        }

        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let impl_arg_ty = ty_from_type_ref_in_context(
            def_map,
            semantic_ir,
            impl_arg,
            context,
            BodyTy::Syntax(impl_arg.clone()),
            &TypeSubst::new(),
        )?;
        if impl_arg_ty != receiver_arg {
            return Ok(false);
        }
    }

    Ok(true)
}

fn semantic_trait_impl_applicability(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    trait_impl: TraitImplRef,
    receiver_ty: &BodyNominalTy,
) -> Result<TraitApplicability, PackageStoreError> {
    let Some(impl_data) = semantic_ir.impl_data(trait_impl.impl_ref)? else {
        return Ok(TraitApplicability::No);
    };
    if !impl_data.resolved_self_tys.contains(&receiver_ty.def)
        || !impl_data
            .resolved_trait_refs
            .contains(&trait_impl.trait_ref)
    {
        return Ok(TraitApplicability::No);
    }

    let header_applicability = if impl_header_is_definitely_direct(impl_data) {
        TraitApplicability::Yes
    } else {
        TraitApplicability::Maybe
    };
    Ok(header_applicability.and(impl_self_args_applicability(
        def_map,
        semantic_ir,
        trait_impl.impl_ref,
        impl_data,
        receiver_ty,
    )?))
}

fn local_impl_self_args_match_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    body_ref: BodyRef,
    body: &BodyData,
    impl_data: &BodyImplData,
    receiver_ty: &BodyLocalNominalTy,
) -> Result<bool, PackageStoreError> {
    // Local impl matching is intentionally shallow. Impl type parameters behave as wildcards;
    // concrete args such as `impl Wrapper<User>` must equal the receiver's known args.
    let TypeRef::Path(self_ty) = &impl_data.self_ty else {
        return Ok(true);
    };
    let Some(segment) = self_ty.segments.last() else {
        return Ok(true);
    };

    let impl_type_args = segment
        .args
        .iter()
        .filter_map(generic_arg_type_ref)
        .collect::<Vec<_>>();
    if impl_type_args.is_empty() {
        return Ok(true);
    }

    let receiver_type_args = receiver_ty
        .args
        .iter()
        .filter_map(body_generic_arg_ty)
        .collect::<Vec<_>>();
    if impl_type_args.len() != receiver_type_args.len() {
        return Ok(false);
    }

    let impl_type_params = impl_type_param_names(&impl_data.generics);
    let resolver = BodyTypePathResolver::new(def_map, semantic_ir, body_ref, body);
    for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
        if type_param_name_from_type_ref(impl_arg)
            .as_deref()
            .is_some_and(|name| impl_type_params.contains(&name))
        {
            continue;
        }

        let impl_arg_ty = resolver.ty_from_type_ref_in_scope_with_subst(
            impl_arg,
            impl_data.scope,
            &TypeSubst::new(),
        )?;
        if impl_arg_ty != receiver_arg {
            return Ok(false);
        }
    }

    Ok(true)
}

fn impl_self_args_applicability(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    impl_ref: ImplRef,
    impl_data: &rg_semantic_ir::ImplData,
    receiver_ty: &BodyNominalTy,
) -> Result<TraitApplicability, PackageStoreError> {
    // This mirrors inherent impl matching, but returns `Maybe` instead of rejecting patterns that
    // contain generic parameters or unsupported pieces we intentionally do not solve.
    let TypeRef::Path(self_ty) = &impl_data.self_ty else {
        return Ok(TraitApplicability::Maybe);
    };
    let Some(segment) = self_ty.segments.last() else {
        return Ok(TraitApplicability::Maybe);
    };

    let impl_type_args = segment
        .args
        .iter()
        .filter_map(generic_arg_type_ref)
        .collect::<Vec<_>>();
    if impl_type_args.is_empty() {
        return Ok(TraitApplicability::Yes);
    }

    let receiver_type_args = receiver_ty
        .args
        .iter()
        .filter_map(body_generic_arg_ty)
        .collect::<Vec<_>>();
    if impl_type_args.len() != receiver_type_args.len() {
        return Ok(TraitApplicability::Maybe);
    }

    let impl_type_params = impl_type_param_names(&impl_data.generics);
    let mut applicability = TraitApplicability::Yes;

    for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
        if impl_arg.mentions_type_param(&impl_type_params) {
            applicability = applicability.and(TraitApplicability::Maybe);
            continue;
        }

        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let impl_arg_ty = ty_from_type_ref_in_context(
            def_map,
            semantic_ir,
            impl_arg,
            context,
            BodyTy::Syntax(impl_arg.clone()),
            &TypeSubst::new(),
        )?;
        if type_arg_comparison_is_uncertain(&impl_arg_ty)
            || type_arg_comparison_is_uncertain(&receiver_arg)
        {
            applicability = applicability.and(TraitApplicability::Maybe);
            continue;
        }

        if impl_arg_ty != receiver_arg {
            return Ok(TraitApplicability::No);
        }
    }

    Ok(applicability)
}

fn impl_type_param_names(generics: &GenericParams) -> Vec<&str> {
    generics
        .types
        .iter()
        .map(|param| param.name.as_str())
        .collect()
}

fn impl_header_is_definitely_direct(impl_data: &rg_semantic_ir::ImplData) -> bool {
    impl_data.generics.lifetimes.is_empty()
        && impl_data.generics.types.is_empty()
        && impl_data.generics.consts.is_empty()
        && impl_data.generics.where_predicates.is_empty()
        && impl_data
            .trait_ref
            .as_ref()
            .is_none_or(|trait_ref| !trait_ref.has_generic_args())
}

fn type_arg_comparison_is_uncertain(ty: &BodyTy) -> bool {
    match ty {
        BodyTy::Syntax(_) | BodyTy::Unknown => true,
        BodyTy::Reference(inner) => type_arg_comparison_is_uncertain(inner),
        BodyTy::Unit
        | BodyTy::Never
        | BodyTy::LocalNominal(_)
        | BodyTy::Nominal(_)
        | BodyTy::SelfTy(_) => false,
    }
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
