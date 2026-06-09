//! Inlay-style hints derived from the frozen analysis snapshot.

use rg_ir_model::TargetRef;
use rg_ir_model::items::{ParamItem, ParamKind};
use rg_ir_storage::ItemStoreQuery;
use rg_ir_view::{display::ty_label::TypeRenderer, ty::locals::BodyView};
use rg_parse::{FileId, TextSpan};

use crate::{
    Analysis,
    model::{InlayHint, InlayHintKind, InlayHintPosition},
};

pub(crate) struct InlayHintCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> InlayHintCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn inlay_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let mut hints = self.binding_type_hints(target, file_id, range)?;
        hints.extend(self.parameter_hints(target, file_id, range)?);

        hints.sort_by_key(|hint| (hint.text_offset(), hint.label.clone()));
        Ok(hints)
    }

    fn binding_type_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        // Binding hints depend on body-level type facts and type rendering, so keep that
        // projection separate from hint families backed by declaration metadata.
        let renderer = TypeRenderer::new(self.0.view_db());
        let mut hints = Vec::new();
        for binding in
            BodyView::new(self.0.view_db()).inferred_binding_tys(target, file_id, range)?
        {
            let Some(ty) = renderer.render(binding.ty())? else {
                continue;
            };

            let hint = InlayHint {
                file_id: binding.file_id(),
                span: binding.span(),
                position: InlayHintPosition::After,
                kind: InlayHintKind::Type,
                label: format!(": {ty}"),
                padding_left: None,
                padding_right: None,
            };
            if !hints.contains(&hint) {
                hints.push(hint);
            }
        }

        Ok(hints)
    }

    fn parameter_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let items = ItemStoreQuery::new(self.0.view_db());
        let mut hints = Vec::new();
        for call in BodyView::new(self.0.view_db()).resolved_function_calls(target, file_id)? {
            let Some(function) = items.function_data(call.function())? else {
                continue;
            };
            let params = function.signature.params();

            for (arg_idx, arg) in call.args().iter().enumerate() {
                let param_idx = arg_idx + call.param_offset();
                let Some(param) = params.get(param_idx) else {
                    continue;
                };
                let Some(param_name) = Self::param_hint_name(param) else {
                    continue;
                };
                let arg_span = arg.span();
                if range.is_some_and(|range| !range.touches(arg_span.text.start)) {
                    continue;
                }
                if self
                    .0
                    .source_text_for_span(target.package, call.file_id(), arg_span)
                    .is_some_and(|arg_text| arg_text.trim() == param_name)
                {
                    continue;
                }

                let hint = InlayHint {
                    file_id: call.file_id(),
                    span: arg_span,
                    position: InlayHintPosition::Before,
                    kind: InlayHintKind::Parameter,
                    label: format!("{param_name}:"),
                    padding_left: None,
                    padding_right: Some(true),
                };
                if !hints.contains(&hint) {
                    hints.push(hint);
                }
            }
        }

        Ok(hints)
    }

    fn param_hint_name(param: &ParamItem) -> Option<&str> {
        if !matches!(param.kind, ParamKind::Normal) {
            return None;
        }

        let name = param.pat.as_str();
        if name == "_" {
            return None;
        }
        let mut chars = name.chars();
        let first = chars.next()?;
        if !(first == '_' || first.is_ascii_alphabetic()) {
            return None;
        }
        chars
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            .then_some(name)
    }
}
