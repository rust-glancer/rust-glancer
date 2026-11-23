//! Inlay-style hints derived from the frozen analysis snapshot.

mod closing_brace;

use std::{borrow::Cow, collections::HashMap};

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::{
    body::BodyStructureView,
    display::ty_label::TypeRenderer,
    member::{MemberFunction, MemberView},
    ty::locals::BodyView,
};
use rg_parse::{SourceReader, syntax_edition};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_syntax::{
    AstNode as _, Edition, LexedStr, SyntaxKind, TextSize,
    ast::{self, HasArgList as _},
};

use super::copied_call;
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
        let mut hints = HintCandidates::default();
        for binding in BodyView::new(self.analysis.view_db())
            .binding_ty_candidates(crate_ref, file_id, range)?
        {
            let source = binding.source();
            let hint = if let Some(ty) = binding.ty() {
                renderer
                    .render_presentation(ty, self.max_length)?
                    .map(|presentation| InlayHint {
                        file_id: source.file_id,
                        span: source.span,
                        position: InlayHintPosition::After,
                        kind: InlayHintKind::Type,
                        label: format!(": {}", presentation.compact),
                        tooltip: Some(presentation.concrete),
                        padding_left: None,
                        padding_right: None,
                    })
            } else {
                None
            };
            hints.record(source.span.end, !source.is_written(), hint);
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
        let mut hints = HintCandidates::default();
        for expr in BodyStructureView::new(self.analysis.view_db())
            .method_chain_expr_tys(crate_ref, file_id, range)?
        {
            let expr_source = expr.source();
            let expr_span = expr_source.span;
            if !range.touches(expr_span.end) {
                continue;
            }
            if !self.should_show_method_chain_expr_hint(
                expr_source.file_id,
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
                file_id: expr_source.file_id,
                span: expr_span,
                position: InlayHintPosition::After,
                kind: InlayHintKind::Type,
                label: presentation.compact,
                tooltip: Some(presentation.concrete),
                padding_left: Some(true),
                padding_right: None,
            };
            hints.record(expr_span.end, !expr_source.is_written(), Some(hint));
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
        let mut hints = HintCandidates::default();
        for call in BodyView::new(self.analysis.view_db())
            .resolved_function_calls(crate_ref, file_id, range)?
        {
            let call_source = call.source();
            let function = match call.target() {
                Some(function) => members
                    .function(*function)
                    .context("read parameter hint function")?,
                None => None,
            };
            let copied = !call_source.is_written();
            let copied_spans;
            let argument_spans = if copied {
                let Some(fragment) = source.text_for_span(file_id, call_source.span)? else {
                    continue;
                };
                let Some(list) = copied_call::argument_list(&fragment, edition) else {
                    continue;
                };
                // In `forward!(parse(make!()))`, the lowered argument may be a token inside
                // `make!()`. Use it to identify enabled arguments and keep the full written
                // argument as the label's anchor.
                let mut lowered = call.argument_spans().iter().copied().peekable();
                copied_spans = list
                    .args()
                    .filter_map(|arg| {
                        let range =
                            arg.syntax().text_range() + TextSize::from(call_source.span.start);
                        let span = Span::from_text_range(range);
                        lowered
                            .next_if(|arg| span.contains_span(*arg))
                            .map(|_| span)
                    })
                    .collect::<Vec<_>>();
                &copied_spans
            } else {
                call.argument_spans()
            };
            for (arg_idx, &span) in argument_spans.iter().enumerate() {
                rg_std::check_cancel!(self.analysis, "inlay hints");
                if !range.touches(span.start) {
                    continue;
                }
                let label = function.and_then(|function| {
                    let param = function.parameter(arg_idx + call.param_offset())?;
                    let name = param.simple_binding_name()?;
                    let is_unary = function.parameters().len() == call.param_offset() + 1;
                    (!param.is_receiver() && Self::show_parameter_name(function, name, is_unary))
                        .then_some(name)
                });
                let label = if let Some(label) = label {
                    let arg_text = source.text_for_span(file_id, span)?;
                    let arg_name = arg_text
                        .as_deref()
                        .and_then(|text| Self::arg_hint_name(text, edition));
                    (!arg_name.as_deref().is_some_and(|name| {
                        Self::arg_name_matches_param_name(name, Self::name_for_comparison(label))
                    }))
                    .then_some(label)
                } else {
                    None
                };

                let hint = label.map(|label| InlayHint {
                    file_id,
                    span,
                    position: InlayHintPosition::Before,
                    kind: InlayHintKind::Parameter,
                    label: format!("{label}:"),
                    tooltip: None,
                    padding_left: None,
                    padding_right: Some(true),
                });
                hints.record(span.start, copied, hint);
            }
        }

        Ok(hints.into_vec())
    }

    // A single input is often explained by the function name (`set_timeout`) or
    // has only a generic role (`value`, `predicate`). Keep these labels when there
    // are several inputs, where they can help distinguish the argument positions.
    fn show_parameter_name(function: MemberFunction<'_>, label: &str, is_unary: bool) -> bool {
        let name = Self::name_for_comparison(label);
        let obvious = function
            .name()
            .strip_suffix(name)
            .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('_'))
            || name.len() == 1
            || ["predicate", "value", "pat", "rhs", "other", "msg", "op"].contains(&name);
        !name.is_empty() && !(is_unary && obvious)
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

/// Show one hint when copies agree at a written position. A copy whose hint is suppressed
/// still supplies an answer, so a label from another copy can disagree with it.
#[derive(Default)]
struct HintCandidates {
    written: UniqueVec<InlayHint>,
    copied: HashMap<u32, ExpectedUnique<Option<InlayHint>>>,
}

impl HintCandidates {
    fn record(&mut self, offset: u32, copied: bool, hint: Option<InlayHint>) {
        if copied {
            self.copied.entry(offset).or_default().push(hint);
        } else if let Some(hint) = hint {
            self.written.push(hint);
        }
    }

    fn into_vec(mut self) -> Vec<InlayHint> {
        for hint in self
            .copied
            .into_values()
            .filter_map(ExpectedUnique::into_option)
            .flatten()
        {
            self.written.push(hint);
        }
        self.written.into_vec()
    }
}
