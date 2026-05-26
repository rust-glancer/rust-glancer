//! Inlay-style hints derived from the frozen analysis snapshot.

use rg_body_ir::{BindingKind, BodyTy};
use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    api::{Analysis, render::ty::TypeRenderer},
    model::TypeHint,
};

pub(crate) struct TypeHintCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeHintCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn type_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<TypeHint>> {
        let renderer = TypeRenderer::new(self.0);
        let mut hints = Vec::new();

        for binding in self.inferred_binding_tys(target, file_id, range)? {
            let Some(ty) = renderer.render(binding.ty())? else {
                continue;
            };

            let hint = TypeHint {
                file_id: binding.file_id(),
                span: binding.span(),
                label: format!(": {ty}"),
            };
            if !hints.contains(&hint) {
                hints.push(hint);
            }
        }

        hints.sort_by_key(|hint| (hint.span.text.start, hint.label.clone()));
        Ok(hints)
    }

    fn inferred_binding_tys(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InferredBindingTy>> {
        let Some(target_bodies) = self.0.body_ir.target_bodies(target)? else {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredBindingTy {
    file_id: FileId,
    span: Span,
    ty: BodyTy,
}

impl InferredBindingTy {
    fn file_id(&self) -> FileId {
        self.file_id
    }

    fn span(&self) -> Span {
        self.span
    }

    fn ty(&self) -> &BodyTy {
        &self.ty
    }
}
