//! Inlay-style hints derived from the frozen analysis snapshot.

use rg_ir_model::TargetRef;
use rg_ir_view::{body::BodyView, ty_label::TypeRenderer};
use rg_parse::{FileId, TextSpan};

use crate::{Analysis, model::TypeHint};

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
        let renderer = TypeRenderer::new(self.0.view_db());
        let mut hints = Vec::new();

        for binding in
            BodyView::new(self.0.view_db()).inferred_binding_tys(target, file_id, range)?
        {
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
}
