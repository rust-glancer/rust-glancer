//! Inlay-style hints derived from the frozen analysis snapshot.

use rg_ir_model::TargetRef;
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
        let renderer = TypeRenderer::new(self.0.view_db());
        let mut hints = Vec::new();

        // Binding hints depend on body-level type facts, unlike syntax-only annotations.
        // Running them through the shared vector keeps deduplication and ordering centralized.
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

        hints.sort_by_key(|hint| (hint.text_offset(), hint.label.clone()));
        Ok(hints)
    }
}
