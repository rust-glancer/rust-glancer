//! Inlay-style hints derived from the frozen analysis snapshot.

mod closing_brace;

use rg_ir_model::{CrateRef, PackageSlot};
use rg_ir_view::{
    body::BodyStructureView,
    display::ty_label::TypeRenderer,
    member::{FunctionParameterView, MemberView},
    ty::locals::BodyView,
};
use rg_parse::{CurrentSource, FileId, Span, TextSpan};
use rg_std::UniqueVec;

use crate::{
    Analysis,
    model::{InlayHint, InlayHintKind, InlayHintPosition},
};

pub(crate) struct InlayHintCollector<'a, 'db>(&'a Analysis<'db>);

/// Source-coordinate operations shared by every inlay-hint family in one file.
///
/// The selected Body IR already determines whether its spans belong to current or saved text. Pick
/// that source once here so individual hint providers cannot accidentally mix the two.
pub(super) enum InlaySource<'a, 'db> {
    Current {
        source: &'a CurrentSource,
        file: FileId,
    },
    Saved {
        analysis: &'a Analysis<'db>,
        package: PackageSlot,
    },
}

impl<'a, 'db> InlaySource<'a, 'db> {
    fn new(analysis: &'a Analysis<'db>, package: PackageSlot, file: FileId) -> Self {
        match analysis.current_source(package, file) {
            Some(source) => Self::Current { source, file },
            None => Self::Saved { analysis, package },
        }
    }

    pub(super) fn current(&self) -> Option<&CurrentSource> {
        match self {
            Self::Current { source, .. } => Some(source),
            Self::Saved { .. } => None,
        }
    }

    pub(super) fn text_for_span(&self, file: FileId, span: Span) -> anyhow::Result<Option<String>> {
        match self {
            Self::Current {
                source,
                file: source_file,
            } if *source_file == file => Ok(source.text_for_span(span).map(ToString::to_string)),
            Self::Current { .. } => Ok(None),
            Self::Saved { analysis, package } => {
                analysis.saved_source_text_for_span(*package, file, span)
            }
        }
    }

    pub(super) fn line_for_offset(&self, file: FileId, offset: u32) -> anyhow::Result<Option<u32>> {
        match self {
            Self::Current {
                source,
                file: source_file,
            } if *source_file == file => Ok(source.line_for_offset(offset)),
            Self::Current { .. } => Ok(None),
            Self::Saved { analysis, package } => {
                analysis.saved_source_line_for_offset(*package, file, offset)
            }
        }
    }
}

impl<'a, 'db> InlayHintCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn inlay_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let source = InlaySource::new(self.0, crate_ref.package, file_id);
        let mut hints = self.binding_type_hints(crate_ref, file_id, range)?;
        hints.extend(self.parameter_hints(crate_ref, file_id, range, &source)?);
        hints.extend(self.expression_type_hints(crate_ref, file_id, range, &source)?);
        hints.extend(closing_brace::closing_brace_hints(
            self.0, crate_ref, file_id, range, &source,
        )?);

        hints.sort_by_key(|hint| (hint.text_offset(), hint.label.clone()));
        Ok(hints)
    }

    fn binding_type_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        // Binding hints depend on body-level type facts and type rendering, so keep that
        // projection separate from hint families backed by declaration metadata.
        let renderer =
            TypeRenderer::new(self.0.view_db(), self.0.view_db().crate_edition(crate_ref)?);
        let mut hints = UniqueVec::new();
        for binding in
            BodyView::new(self.0.view_db()).inferred_binding_tys(crate_ref, file_id, range)?
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
            hints.push(hint);
        }

        Ok(hints.into_vec())
    }

    fn expression_type_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Option<TextSpan>,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let renderer =
            TypeRenderer::new(self.0.view_db(), self.0.view_db().crate_edition(crate_ref)?);
        let mut hints = UniqueVec::new();
        for expr in
            BodyStructureView::new(self.0.view_db()).method_chain_expr_tys(crate_ref, file_id)?
        {
            let expr_span = expr.span();
            if range.is_some_and(|range| !range.touches(expr_span.text.end)) {
                continue;
            }
            if !self.should_show_method_chain_expr_hint(
                expr.file_id(),
                expr_span,
                expr.parent_dot_span(),
                source,
            )? {
                continue;
            }
            if expr.ty().is_unit_or_never() {
                continue;
            }
            let Some(ty) = renderer.render(expr.ty())? else {
                continue;
            };

            let hint = InlayHint {
                file_id: expr.file_id(),
                span: expr_span,
                position: InlayHintPosition::After,
                kind: InlayHintKind::Type,
                label: ty,
                padding_left: Some(true),
                padding_right: None,
            };
            hints.push(hint);
        }

        Ok(hints.into_vec())
    }

    fn parameter_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Option<TextSpan>,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let members = MemberView::new(self.0.view_db());
        let mut hints = UniqueVec::new();
        for call in BodyView::new(self.0.view_db()).resolved_function_calls(crate_ref, file_id)? {
            let Some(function) = members.function(call.function())? else {
                continue;
            };
            for (arg_idx, arg) in call.args().iter().enumerate() {
                let param_idx = arg_idx + call.param_offset();
                let Some(param) = function.parameter(param_idx) else {
                    continue;
                };
                let Some(param_name) = Self::param_hint_name(param) else {
                    continue;
                };
                let arg_span = arg.span();
                if range.is_some_and(|range| !range.touches(arg_span.text.start)) {
                    continue;
                }
                let arg_text = source.text_for_span(call.file_id(), arg_span)?;
                if arg_text.is_some_and(|arg_text| arg_text.trim() == param_name) {
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
                hints.push(hint);
            }
        }

        Ok(hints.into_vec())
    }

    fn param_hint_name(param: FunctionParameterView<'_>) -> Option<&str> {
        if param.is_receiver() {
            return None;
        }

        let name = param.pattern();
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

    fn should_show_method_chain_expr_hint(
        &self,
        file_id: FileId,
        expr_span: rg_parse::Span,
        parent_dot_span: rg_parse::Span,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<bool> {
        let expr_end_offset = expr_span.text.end.saturating_sub(1);
        let expr_end_line = source.line_for_offset(file_id, expr_end_offset)?;
        let Some(expr_end_line) = expr_end_line else {
            return Ok(false);
        };
        let parent_dot_line = source.line_for_offset(file_id, parent_dot_span.text.start)?;
        let Some(parent_dot_line) = parent_dot_line else {
            return Ok(false);
        };

        Ok(parent_dot_line > expr_end_line)
    }
}
