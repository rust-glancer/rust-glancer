//! Inference-aware member projection for fields and indexing.
//!
//! This layer turns `base.field` and `base[index]` into inference facts that still share vars with
//! `base`, so later evidence on the projected value can solve the owner.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, items::FieldKey};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::Ty;

use crate::{ir::ExprKind, resolution::BodyResolutionContext};

use super::BodyInferenceCtx;

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

    /// Project a field or index expression from its current base inference fact.
    pub(crate) fn project_expr(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
    ) -> Result<(), PackageStoreError> {
        let kind = self.context.body().expr_unchecked(expr).kind.clone();
        match kind {
            ExprKind::Field {
                base: Some(base),
                field: Some(field),
                ..
            } => self.project_field(inference, expr, base, &field),
            ExprKind::Index {
                base: Some(base), ..
            } => {
                self.project_index(inference, expr, base);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Project `boxed.value` as `?T` when `boxed` is `Boxed<?T>`.
    fn project_field(
        &self,
        inference: &mut BodyInferenceCtx,
        expr: ExprId,
        base: ExprId,
        field: &FieldKey,
    ) -> Result<(), PackageStoreError> {
        let base_ty = inference.root_resolved_expr_ty(base);
        let targets = self.context.fields().resolve_for_ty(&base_ty, field)?;
        let Some(projected_ty) = targets.single_ty() else {
            return Ok(());
        };
        // Field lookup already owns receiver adjustment and applies the selected candidate's live
        // arguments. Reconstructing structural or nominal owners here would create a second,
        // less-capable autoderef path.
        inference.set_expr_infer_ty(expr, projected_ty.clone());
        Ok(())
    }

    /// Project `array[index]` as the element type, peeling explicit references.
    fn project_index(&self, inference: &mut BodyInferenceCtx, expr: ExprId, base: ExprId) {
        let base_ty = inference.root_resolved_expr_ty(base);
        let Some(element_ty) = Self::structural_index_ty(&base_ty) else {
            return;
        };
        let element_ty = inference.root_resolved_ty(&element_ty);

        inference.set_expr_infer_ty(expr, element_ty);
    }

    /// Return the element type for array/slice indexing.
    fn structural_index_ty(ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Array { inner, .. } | Ty::Slice(inner) => Some(inner.as_ref().clone()),
            Ty::Reference { inner, .. } => Self::structural_index_ty(inner),
            _ => None,
        }
    }
}
