//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod autoderef;
mod body;
mod deref;
mod impl_match;
mod index;
mod method;
mod normalize;
mod pat;
mod ty;
mod type_path;

use rg_def_map::{DefMapReadTxn, Path};
use rg_ir_model::{FieldRef, FunctionRef, TraitApplicability, TraitImplRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{SemanticIrReadTxn, TypePathContext};
use rg_ty::{IndexedLocalNominalTy, IndexedNominalTy, IndexedTy};

use crate::{
    BodyData, BodyResolution,
    ir::ids::{BodyFunctionRef, BodyRef, ScopeId},
    ir::resolved::BodyTypePathResolution,
};

use self::{
    body::BodyValuePathResolver,
    impl_match::BodyImplMatcher,
    method::{
        local_function_applies_to_receiver as local_function_applies_to_receiver_impl,
        semantic_function_applies_to_receiver as semantic_function_applies_to_receiver_impl,
        semantic_trait_function_candidates_for_receiver as semantic_trait_function_candidates_for_receiver_impl,
    },
    ty::{TypeSubst, ty_from_type_ref_in_context},
    type_path::BodyTypePathResolver,
};

pub use self::autoderef::{
    BodyAutoderef, BodyAutoderefCandidate, BodyAutoderefCandidates, BodyAutoderefMode,
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
) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
    let Some(body) = body else {
        return Ok((BodyResolution::Unknown, IndexedTy::Unknown));
    };

    BodyValuePathResolver::new(def_map, semantic_ir, None, body_ref, body)
        .resolve_nonlocal_path_expr(scope, path)
}

pub(super) fn ty_for_field(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    field_ref: FieldRef,
) -> Result<Option<IndexedTy>, PackageStoreError> {
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
        IndexedTy::Unknown,
        &TypeSubst::new(),
    )?))
}

pub(super) fn semantic_function_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: FunctionRef,
    receiver_ty: &IndexedNominalTy,
) -> Result<bool, PackageStoreError> {
    semantic_function_applies_to_receiver_impl(def_map, semantic_ir, function_ref, receiver_ty)
}

pub(super) fn semantic_trait_function_candidates_for_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    receiver_ty: &IndexedNominalTy,
) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
    semantic_trait_function_candidates_for_receiver_impl(
        None,
        def_map,
        semantic_ir,
        receiver_ty,
        None,
    )
}

pub(super) fn semantic_trait_impl_applies_to_receiver(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    trait_impl: TraitImplRef,
    receiver_ty: &IndexedNominalTy,
) -> Result<bool, PackageStoreError> {
    Ok(BodyImplMatcher::new(def_map, semantic_ir)
        .semantic_trait_impl_applicability(trait_impl, receiver_ty)?
        .is_applicable())
}

pub(super) fn local_function_applies_to_receiver(
    body: Option<&BodyData>,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    function_ref: BodyFunctionRef,
    receiver_ty: &IndexedLocalNominalTy,
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
