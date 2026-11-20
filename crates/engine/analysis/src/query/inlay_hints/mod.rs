//! Inlay-style hints derived from the frozen analysis snapshot.

mod closing_brace;

use std::borrow::Cow;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::{
    body::BodyStructureView, display::ty_label::TypeRenderer, member::MemberView,
    ty::locals::BodyView,
};
use rg_parse::{SourceReader, syntax_edition};
use rg_std::UniqueVec;
use rg_syntax::{
    AstNode as _, Edition, LexedStr, SyntaxKind,
    ast::{self, HasArgList as _},
};

use crate::{
    Analysis,
    model::{InlayHint, InlayHintKind, InlayHintPosition},
};

pub(crate) struct InlayHintCollector<'a, 'db> {
    analysis: &'a Analysis<'db>,
    max_length: Option<usize>,
}

impl<'a, 'db> InlayHintCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, max_length: Option<usize>) -> Self {
        Self {
            analysis,
            max_length,
        }
    }

    pub(crate) fn inlay_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Span,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let source = self.analysis.sources;
        let edition = self.analysis.view_db().crate_edition(crate_ref)?;
        let renderer = TypeRenderer::new(self.analysis.view_db(), edition)
            .with_iterator_simplification(crate_ref);
        let mut hints = self.binding_type_hints(crate_ref, file_id, range, &renderer)?;
        hints.extend(self.parameter_hints(crate_ref, file_id, range, &source)?);
        hints.extend(self.expression_type_hints(crate_ref, file_id, range, &source, &renderer)?);
        hints.extend(closing_brace::closing_brace_hints(
            self.analysis,
            crate_ref,
            file_id,
            range,
            &source,
        )?);

        hints.sort_by_key(|hint| (hint.text_offset(), hint.label.clone()));
        Ok(hints)
    }

    fn binding_type_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Span,
        renderer: &TypeRenderer<'_, '_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        // Binding hints depend on body-level type facts and type rendering, so keep that
        // projection separate from hint families backed by declaration metadata.
        let mut hints = UniqueVec::new();
        for binding in BodyView::new(self.analysis.view_db())
            .inferred_binding_tys(crate_ref, file_id, range)?
        {
            let Some(presentation) = renderer.render_presentation(binding.ty(), self.max_length)?
            else {
                continue;
            };

            let hint = InlayHint {
                file_id: binding.file_id(),
                span: binding.span(),
                position: InlayHintPosition::After,
                kind: InlayHintKind::Type,
                label: format!(": {}", presentation.compact),
                tooltip: Some(presentation.concrete),
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
        range: Span,
        source: &SourceReader<'_>,
        renderer: &TypeRenderer<'_, '_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let mut hints = UniqueVec::new();
        for expr in BodyStructureView::new(self.analysis.view_db())
            .method_chain_expr_tys(crate_ref, file_id, range)?
        {
            let expr_span = expr.span();
            if !range.touches(expr_span.end) {
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
            let Some(presentation) = renderer.render_presentation(expr.ty(), self.max_length)?
            else {
                continue;
            };

            let hint = InlayHint {
                file_id: expr.file_id(),
                span: expr_span,
                position: InlayHintPosition::After,
                kind: InlayHintKind::Type,
                label: presentation.compact,
                tooltip: Some(presentation.concrete),
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
        range: Span,
        source: &SourceReader<'_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let members = MemberView::new(self.analysis.view_db());
        let edition = syntax_edition(self.analysis.view_db().crate_edition(crate_ref)?);
        let mut hints = UniqueVec::new();
        for call in BodyView::new(self.analysis.view_db())
            .resolved_function_calls(crate_ref, file_id, range)?
        {
            let Some(function) = members
                .function(call.function())
                .context("read parameter hint function")?
            else {
                continue;
            };
            let is_unary = function.parameters().len() == call.param_offset() + 1;
            for (arg_idx, arg) in call.args().iter().enumerate() {
                rg_std::check_cancel!(self.analysis, "inlay hints");
                let param_idx = arg_idx + call.param_offset();
                let Some(param) = function
                    .parameter(param_idx)
                    .filter(|param| !param.is_receiver())
                else {
                    continue;
                };
                let Some(param_name) = param.simple_binding_name() else {
                    continue;
                };

                let name = Self::name_for_comparison(param_name);
                if name.is_empty()
                    || (is_unary && Self::is_obvious_param_name(function.name(), name))
                {
                    continue;
                }
                let arg_span = arg.span();
                if !range.touches(arg_span.start) {
                    continue;
                }
                let arg_text = source.text_for_span(call.file_id(), arg_span)?;
                let arg_name = arg_text
                    .as_deref()
                    .and_then(|text| Self::arg_hint_name(text, edition));
                if arg_name
                    .as_deref()
                    .is_some_and(|arg_name| Self::arg_name_matches_param_name(arg_name, name))
                {
                    continue;
                }

                let hint = InlayHint {
                    file_id: call.file_id(),
                    span: arg_span,
                    position: InlayHintPosition::Before,
                    kind: InlayHintKind::Parameter,
                    label: format!("{param_name}:"),
                    tooltip: None,
                    padding_left: None,
                    padding_right: Some(true),
                };
                hints.push(hint);
            }
        }

        Ok(hints.into_vec())
    }

    // A single input is often explained by the function name (`set_timeout`) or
    // has only a generic role (`value`, `predicate`). Keep these labels when there
    // are several inputs, where they can help distinguish the argument positions.
    fn is_obvious_param_name(function_name: &str, name: &str) -> bool {
        function_name
            .strip_suffix(name)
            .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('_'))
            || name.len() == 1
            || ["predicate", "value", "pat", "rhs", "other", "msg", "op"].contains(&name)
    }

    // Recover the name already visible in `&user`, `user.clone()`, or `model.user`.
    // A plain identifier needs only lexing; wrappers and paths need the argument's syntax tree.
    fn arg_hint_name(text: &str, edition: Edition) -> Option<Cow<'_, str>> {
        let text = text.trim();
        if matches!(
            LexedStr::single_token(edition, text),
            Some((SyntaxKind::IDENT, None))
        ) {
            return Some(Cow::Borrowed(text));
        }
        let parsed = ast::Expr::parse(text, edition);
        let mut expr = ast::Expr::cast(parsed.syntax_node())?;
        loop {
            expr = match expr {
                ast::Expr::PathExpr(path) => {
                    return Some(Cow::Owned(
                        path.path()?.segment()?.name_ref()?.text().to_string(),
                    ));
                }
                ast::Expr::FieldExpr(field) => {
                    return Some(Cow::Owned(field.name_ref()?.text().to_string()));
                }
                ast::Expr::MethodCallExpr(method) => {
                    let method_name = method.name_ref()?.text().to_string();
                    if ["clone", "as_ref", "into"].contains(&method_name.as_str())
                        && method.arg_list()?.args().next().is_none()
                    {
                        method.receiver()?
                    } else {
                        return Some(Cow::Owned(method_name));
                    }
                }
                ast::Expr::RefExpr(reference) => reference.expr()?,
                ast::Expr::ParenExpr(paren) => paren.expr()?,
                ast::Expr::CallExpr(call) => call.expr()?,
                _ => return None,
            };
        }
    }

    // Names such as `local_user` and `user_id` already expose a `user` parameter's role.
    fn arg_name_matches_param_name(arg_name: &str, name: &str) -> bool {
        let arg_name = Self::name_for_comparison(arg_name);
        arg_name.eq_ignore_ascii_case(name)
            || arg_name
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('_'))
            || arg_name
                .strip_suffix(name)
                .is_some_and(|rest| rest.ends_with('_'))
    }

    fn name_for_comparison(name: &str) -> &str {
        name.strip_prefix("r#").unwrap_or(name).trim_matches('_')
    }

    fn should_show_method_chain_expr_hint(
        &self,
        file_id: FileId,
        expr_span: Span,
        parent_dot_span: Span,
        source: &SourceReader<'_>,
    ) -> anyhow::Result<bool> {
        let expr_end_offset = expr_span.end.saturating_sub(1);
        let index = source.line_index(file_id)?;
        let expr_end_line = index.position(expr_end_offset).line;
        let parent_dot_line = index.position(parent_dot_span.start).line;

        Ok(parent_dot_line > expr_end_line)
    }
}
