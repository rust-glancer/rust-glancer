//! Inference-aware structural pattern projection.
//!
//! This layer links bindings inside tuple, reference, and slice patterns to the matching
//! initializer slots, so later evidence on the binding can solve the initializer.

use rg_ir_model::{ExprId, PatId};
use rg_ty::{RefMutability, inference::InferTy};

use crate::{
    ir::{PatKind, PatMutability},
    resolution::BodyResolutionContext,
};

use super::BodyInferenceCtx;

/// Links structural pattern bindings to initializer inference facts.
pub(crate) struct BodyPatternInference<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyPatternInference<'query, D, I> {
    /// Build pattern inference from a read-only body resolution context.
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Link `let pat = initializer` where `pat` is structurally visible in the initializer type.
    pub(crate) fn link_initializer_pattern(
        &self,
        inference: &mut BodyInferenceCtx,
        pat: PatId,
        initializer: ExprId,
    ) -> bool {
        let initializer_ty = inference.root_resolved_expr_ty(initializer);
        self.link_pat(inference, pat, &initializer_ty)
    }

    /// Project the current pattern from its matching initializer slot.
    fn link_pat(&self, inference: &mut BodyInferenceCtx, pat: PatId, ty: &InferTy) -> bool {
        let ty = inference.root_resolved_ty(ty);
        let Some(data) = self.context.body().pat(pat).cloned() else {
            return false;
        };

        match data.kind {
            PatKind::Binding {
                binding, subpat, ..
            } => {
                let mut changed = false;
                if let Some(binding) = binding {
                    changed |= inference.set_binding_infer_ty(binding, ty.clone());
                }
                if let Some(subpat) = subpat {
                    changed |= self.link_pat(inference, subpat, &ty);
                }
                changed
            }
            PatKind::Tuple { fields } => self.link_tuple_pat(inference, &fields, &ty),
            PatKind::Slice { fields } => self.link_slice_pat(inference, &fields, &ty),
            PatKind::Or { pats } => {
                let mut changed = false;
                for pat in pats {
                    changed |= self.link_pat(inference, pat, &ty);
                }
                changed
            }
            PatKind::Ref { mutability, pat } => self.link_ref_pat(inference, pat, mutability, &ty),
            PatKind::TupleStruct { .. }
            | PatKind::Record { .. }
            | PatKind::Box { .. }
            | PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => false,
        }
    }

    /// Link tuple fields by position, e.g. `(values,) = (Vec::new(),)`.
    fn link_tuple_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        fields: &[PatId],
        ty: &InferTy,
    ) -> bool {
        let InferTy::Tuple(field_tys) = ty else {
            return false;
        };
        if fields.len() != field_tys.len() {
            return false;
        }

        let mut changed = false;
        for (field_pat, field_ty) in fields.iter().zip(field_tys) {
            changed |= self.link_pat(inference, *field_pat, field_ty);
        }
        changed
    }

    /// Link `&pat` to the referenced slot when the initializer is explicitly a reference.
    fn link_ref_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        pat: PatId,
        pat_mutability: PatMutability,
        ty: &InferTy,
    ) -> bool {
        let InferTy::Reference { mutability, inner } = ty else {
            return false;
        };
        if *mutability != Self::ref_mutability(pat_mutability) {
            return false;
        }

        self.link_pat(inference, pat, inner)
    }

    /// Link every non-rest slice pattern field to the element slot.
    fn link_slice_pat(
        &self,
        inference: &mut BodyInferenceCtx,
        fields: &[PatId],
        ty: &InferTy,
    ) -> bool {
        let element_ty = match ty {
            InferTy::Array { inner, .. } | InferTy::Slice(inner) => inner.as_ref(),
            _ => return false,
        };
        let element_ty = inference.root_resolved_ty(element_ty);

        let mut changed = false;
        for field in fields {
            if self.pat_is_rest(*field) {
                continue;
            }
            changed |= self.link_pat(inference, *field, &element_ty);
        }
        changed
    }

    fn pat_is_rest(&self, pat: PatId) -> bool {
        self.context
            .body()
            .pat(pat)
            .is_some_and(|pat| matches!(&pat.kind, PatKind::Rest))
    }

    fn ref_mutability(mutability: PatMutability) -> RefMutability {
        match mutability {
            PatMutability::Shared => RefMutability::Shared,
            PatMutability::Mut => RefMutability::Mutable,
        }
    }
}
