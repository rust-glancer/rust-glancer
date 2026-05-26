//! Source facts for inferred local binding type hints.

use rg_body_ir::{BindingKind, BodyTy};
use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span, TextSpan};

use crate::api::Analysis;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InferredBindingTy {
    file_id: FileId,
    span: Span,
    ty: BodyTy,
}

impl InferredBindingTy {
    pub(crate) fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn ty(&self) -> &BodyTy {
        &self.ty
    }
}

pub(crate) struct TypeHintView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> TypeHintView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn inferred_binding_tys(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InferredBindingTy>> {
        let Some(target_bodies) = self.analysis.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for body in target_bodies.bodies() {
            for binding in body.bindings() {
                if binding.source.file_id != file_id {
                    continue;
                }
                if !matches!(binding.kind, BindingKind::Let) {
                    continue;
                }
                if binding.name.is_none() || binding.annotation.is_some() {
                    continue;
                }
                if matches!(binding.ty, BodyTy::Unknown) {
                    continue;
                }
                if range.is_some_and(|range| !range.touches(binding.source.span.text.end)) {
                    continue;
                }

                bindings.push(InferredBindingTy {
                    file_id: binding.source.file_id,
                    span: binding.source.span,
                    ty: binding.ty.clone(),
                });
            }
        }

        Ok(bindings)
    }
}
