//! Inference-aware member projection for fields and indexing.
//!
//! This layer turns `base.field` and `base[index]` into inference facts that still share vars with
//! `base`, so later evidence on the projected value can solve the owner.

use rg_ir_model::{
    ExprId,
    items::{FieldKey, GenericParams},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{NominalTy, Ty, inference::InferTy};

use crate::{ir::ExprKind, resolution::BodyResolutionContext};

use super::{BodyInferenceCtx, InferTypeRefProjector, InferTypeSubst};

/// Projects member expressions while preserving inference variables from the base.
pub(crate) struct BodyMemberInference<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyMemberInference<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Build member inference from a read-only body resolution context.
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Refresh a field or index expression from its base inference fact.
    pub(crate) fn refresh_projection_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
    ) -> Result<bool, PackageStoreError> {
        let kind = self.context.body().expr_unchecked(expr).kind.clone();
        match kind {
            ExprKind::Field {
                base: Some(base),
                field: Some(field),
                ..
            } => self.refresh_field_fact(inference, expr, base, &field),
            ExprKind::Index {
                base: Some(base), ..
            } => Ok(self.refresh_index_fact(inference, expr, base)),
            _ => Ok(false),
        }
    }

    /// Project `boxed.value` as `?T` when `boxed` is `Boxed<?T>`.
    fn refresh_field_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
        base: ExprId,
        field: &FieldKey,
    ) -> Result<bool, PackageStoreError> {
        let base_ty = inference.root_resolved_expr_ty(base);
        if let Some(field_ty) = Self::structural_field_ty(&base_ty, field) {
            return Ok(inference.set_expr_infer_ty(expr, field_ty));
        }

        self.refresh_declared_field_fact(inference, expr, base, field)
    }

    /// Project a declared field type through the base owner's inference vars.
    fn refresh_declared_field_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
        base: ExprId,
        field: &FieldKey,
    ) -> Result<bool, PackageStoreError> {
        let targets = self.context.fields().resolve(base, field)?;
        let Some(target) = targets.single_declared() else {
            return Ok(false);
        };
        let Some(field_ty_ref) = target.ty_ref() else {
            return Ok(false);
        };
        let fallback_ty = target.ty().cloned().unwrap_or(Ty::Unknown);

        let Some(generics) = self
            .context
            .item_query()
            .generic_params_for_type_def(target.owner_ty().def)?
            .cloned()
        else {
            return Ok(inference.set_expr_infer_ty(expr, InferTy::from_ty(&fallback_ty)));
        };

        let Some(subst) = self.infer_subst_for_owner(inference, base, target.owner_ty(), &generics)
        else {
            return Ok(false);
        };

        let projected_ty =
            InferTypeRefProjector::new(&subst).ty_from_type_ref(field_ty_ref, &fallback_ty);
        Ok(inference.set_expr_infer_ty(expr, projected_ty))
    }

    /// Bind owner generics from `Boxed<?T>` before projecting `field: T`.
    fn infer_subst_for_owner(
        &self,
        inference: &mut BodyInferenceCtx,
        base: ExprId,
        owner_ty: &NominalTy,
        generics: &GenericParams,
    ) -> Option<InferTypeSubst> {
        let base_ty = inference.root_resolved_expr_ty(base);
        let infer_args = match base_ty {
            InferTy::Nominal(ty) | InferTy::SelfTy(ty) if ty.def == owner_ty.def => ty.args,
            _ => return None,
        };

        let mut subst = InferTypeSubst::new();
        subst.bind_type_params_from_infer_args(inference, generics, &infer_args);
        Some(subst)
    }

    /// Project `pair.0` from an inference-aware tuple, peeling explicit references.
    fn structural_field_ty(ty: &InferTy, field: &FieldKey) -> Option<InferTy> {
        match (ty, field) {
            (InferTy::Tuple(fields), FieldKey::Tuple(index)) => fields.get(*index).cloned(),
            (InferTy::Reference { inner, .. }, _) => Self::structural_field_ty(inner, field),
            _ => None,
        }
    }

    /// Project `array[index]` as the element type, peeling explicit references.
    fn refresh_index_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
        base: ExprId,
    ) -> bool {
        let base_ty = inference.root_resolved_expr_ty(base);
        let Some(element_ty) = Self::structural_index_ty(&base_ty) else {
            return false;
        };

        inference.set_expr_infer_ty(expr, element_ty)
    }

    /// Return the element type for array/slice indexing.
    fn structural_index_ty(ty: &InferTy) -> Option<InferTy> {
        match ty {
            InferTy::Array { inner, .. } | InferTy::Slice(inner) => Some(inner.as_ref().clone()),
            InferTy::Reference { inner, .. } => Self::structural_index_ty(inner),
            _ => None,
        }
    }
}
