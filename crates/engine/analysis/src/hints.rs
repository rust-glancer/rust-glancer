//! Inlay-style hints derived from the frozen analysis snapshot.

use rg_body_ir::{BindingKind, BodyTy};
use rg_def_map::TargetRef;
use rg_parse::{FileId, TextSpan};

use super::{Analysis, data::TypeHint, type_render::TypeRenderer};

pub(super) struct TypeHintCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeHintCollector<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn type_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<TypeHint>> {
        let Some(target_bodies) = self.0.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let renderer = TypeRenderer::new(self.0);
        let mut hints = Vec::new();

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

                let Some(ty) = renderer.render(&binding.ty)? else {
                    continue;
                };

                let hint = TypeHint {
                    file_id,
                    span: binding.source.span,
                    label: format!(": {ty}"),
                };
                if !hints.contains(&hint) {
                    hints.push(hint);
                }
            }
        }

        hints.sort_by_key(|hint| (hint.span.text.start, hint.label.clone()));
        Ok(hints)
    }
}
