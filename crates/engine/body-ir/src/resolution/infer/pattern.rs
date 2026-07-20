//! Inference-aware pattern projection.
//!
//! Every source of an expected pattern type eventually enters through [`BodyPatternInference`].
//! It projects structural and nominal fields while writing bindings into the one live inference
//! context, so closure obligations and ordinary body traversal cannot disagree about patterns.

use rg_def_map::DefMapSource;
use rg_ir_model::{FieldKey, Mutability, PatId};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::Ty;

use crate::{
    ir::{BodyPath, PatKind, RecordPatField},
    resolution::BodyResolutionContext,
};

use super::BodyInferenceCtx;

/// Projects one expected type through a pattern into its binding slots.
pub(crate) struct BodyPatternInference<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyPatternInference<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Link the complete pattern to one live expected type.
    pub(crate) fn link_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        pat: PatId,
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let expected_ty = inference.root_resolved_ty(expected_ty);
        if matches!(expected_ty, Ty::Unknown) {
            return Ok(());
        }

        let Some(data) = self.context.body().pat(pat).cloned() else {
            return Ok(());
        };

        match data.kind {
            PatKind::Binding {
                binding, subpat, ..
            } => {
                if let Some(binding) = binding {
                    inference.set_binding_infer_ty(binding, expected_ty.clone());
                }
                if let Some(subpat) = subpat {
                    self.link_pat(inference, subpat, &expected_ty)?;
                }
                Ok(())
            }
            PatKind::TupleStruct { path, fields } => {
                self.link_tuple_variant(inference, path.as_ref(), &fields, &expected_ty)
            }
            PatKind::Record { path, fields, .. } => {
                self.link_record_pat(inference, path.as_ref(), &fields, &expected_ty)
            }
            PatKind::Tuple { fields } => self.link_tuple_pat(inference, &fields, &expected_ty),
            PatKind::Slice { fields } => self.link_slice_pat(inference, &fields, &expected_ty),
            PatKind::Or { pats } => {
                for pat in pats {
                    self.link_pat(inference, pat, &expected_ty)?;
                }
                Ok(())
            }
            PatKind::Ref { mutability, pat } => {
                self.link_ref_pat(inference, pat, mutability, &expected_ty)
            }
            PatKind::Box { pat } => self.link_pat(inference, pat, &expected_ty),
            PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => Ok(()),
        }
    }

    /// Project tuple fields by position, e.g. `(left, right): (User, bool)`.
    fn link_tuple_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let Ty::Tuple(field_tys) = expected_ty else {
            return Ok(());
        };
        if fields.len() != field_tys.len() {
            return Ok(());
        }

        for (field_pat, field_ty) in fields.iter().zip(field_tys) {
            self.link_pat(inference, *field_pat, field_ty)?;
        }
        Ok(())
    }

    /// Give every non-rest slice pattern the container's element type.
    fn link_slice_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let element_ty = match expected_ty {
            Ty::Array { inner, .. } | Ty::Slice(inner) => inner.as_ref(),
            _ => return Ok(()),
        };

        for field in fields {
            if self
                .context
                .body()
                .pat(*field)
                .is_some_and(|pat| matches!(&pat.kind, PatKind::Rest))
            {
                continue;
            }
            self.link_pat(inference, *field, element_ty)?;
        }
        Ok(())
    }

    /// Peel only the reference written by the pattern, preserving its mutability contract.
    fn link_ref_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        pat: PatId,
        pat_mutability: Mutability,
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let Some((inner_ty, mutability)) = expected_ty.reference_inner() else {
            return Ok(());
        };
        if mutability != pat_mutability {
            return Ok(());
        }

        self.link_pat(inference, pat, inner_ty)
    }

    /// Project tuple-variant payload fields from the expected enum instantiation.
    fn link_tuple_variant(
        &self,
        inference: &mut BodyInferenceCtx,
        path: Option<&BodyPath>,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        for (index, field_pat) in fields.iter().enumerate() {
            let field_key = FieldKey::Tuple(index);
            if let Some(field_ty) =
                self.context
                    .fields()
                    .pattern_field_ty(path, expected_ty, &field_key)?
            {
                self.link_pat(inference, *field_pat, &field_ty)?;
            }
        }
        Ok(())
    }

    /// Project named pattern fields from structs, unions, or record enum variants.
    fn link_record_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        path: Option<&BodyPath>,
        fields: &[RecordPatField],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        for field in fields {
            if let Some(field_ty) =
                self.context
                    .fields()
                    .pattern_field_ty(path, expected_ty, &field.key)?
            {
                self.link_pat(inference, field.pat, &field_ty)?;
            }
        }
        Ok(())
    }
}
