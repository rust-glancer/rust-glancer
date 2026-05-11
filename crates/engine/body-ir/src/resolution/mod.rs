//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod body;
mod index;
mod method;
mod normalize;
mod pat;
mod ty;
mod type_path;

use rg_def_map::{DefMapReadTxn, Path};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{
    FieldRef, FunctionRef, SemanticIrReadTxn, TraitApplicability, TypePathContext,
};

use crate::{
    BodyData, BodyResolution,
    ids::{BodyFunctionRef, BodyRef, ScopeId},
    resolved::BodyTypePathResolution,
    ty::{BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

use self::{
    body::BodyValuePathResolver,
    method::{
        local_function_applies_to_receiver as local_function_applies_to_receiver_impl,
        semantic_function_applies_to_receiver as semantic_function_applies_to_receiver_impl,
        semantic_trait_function_candidates_for_receiver as semantic_trait_function_candidates_for_receiver_impl,
    },
    ty::{TypeSubst, ty_from_type_ref_in_context},
    type_path::BodyTypePathResolver,
};

pub(crate) use self::body::BodyResolver;
pub(crate) use self::index::SemanticResolutionIndex;

pub(super) fn resolve_type_path_in_scope(
    body: Option<&BodyData>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    body_ref: BodyRef,
    scope: ScopeId,
    path: &Path,
) -> Result<BodyTypePathResolution, PackageStoreError> {
    let Some(body) = body else {
        return Ok(BodyTypePathResolution::Unknown);
    };

    BodyTypePathResolver::new(def_map, semantic_ir, body_ref, body).resolve_in_scope(scope, path)
}

pub(super) fn resolve_value_path_in_scope(
    body: Option<&BodyData>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    body_ref: BodyRef,
    scope: ScopeId,
    path: &Path,
) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
    let Some(body) = body else {
        return Ok((BodyResolution::Unknown, BodyTy::Unknown));
    };

    BodyValuePathResolver::new(def_map, semantic_ir, None, body_ref, body)
        .resolve_nonlocal_path_expr(scope, path)
}

pub(super) fn ty_for_field(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    field_ref: FieldRef,
) -> Result<Option<BodyTy>, PackageStoreError> {
    // Field declarations live in Semantic IR, but Analysis expects Body IR's small type
    // vocabulary. Use the owning module as the type-path context for the field signature.
    let Some(field_data) = semantic_ir.field_data(field_ref)? else {
        return Ok(None);
    };
    Ok(Some(ty_from_type_ref_in_context(
        def_map,
        semantic_ir,
        &field_data.field.ty,
        TypePathContext::module(field_data.owner_module),
        BodyTy::Unknown,
        &TypeSubst::new(),
    )?))
}

pub(super) fn semantic_function_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: FunctionRef,
    receiver_ty: &BodyNominalTy,
) -> Result<bool, PackageStoreError> {
    semantic_function_applies_to_receiver_impl(def_map, semantic_ir, function_ref, receiver_ty)
}

pub(super) fn semantic_trait_function_candidates_for_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    receiver_ty: &BodyNominalTy,
) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
    semantic_trait_function_candidates_for_receiver_impl(
        None,
        def_map,
        semantic_ir,
        receiver_ty,
        None,
    )
}

pub(super) fn local_function_applies_to_receiver(
    body: Option<&BodyData>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: BodyFunctionRef,
    receiver_ty: &BodyLocalNominalTy,
) -> Result<bool, PackageStoreError> {
    let Some(body) = body else {
        return Ok(false);
    };
    local_function_applies_to_receiver_impl(
        def_map,
        semantic_ir,
        function_ref.body,
        body,
        function_ref,
        receiver_ty,
    )
}

pub(super) fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    // Resolution often merges candidates from local, inherent, and trait sources. Keeping order
    // while deduplicating makes snapshots stable without pretending this is a ranking policy.
    if !items.contains(&item) {
        items.push(item);
    }
}
