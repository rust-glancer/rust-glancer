//! Find caller-written format strings by following macro expansions.
//!
//! A call such as `render!("{user}")` may forward its arguments through several wrappers before
//! reaching `format_args!`. Follow those expansions to identify the format template, then use
//! the token maps to recover the literal's position in the original file.
//!
//! Highlighting only needs that position and the placeholder syntax. Preparing analyzed function
//! bodies would also resolve names and infer types, which this query does not need.

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{BodySource, BuiltinMacroExprKind, CrateRef, FileId, Span};
use rg_parse::{Package, enclosing_inline_module_path};
use rg_syntax::{AstNode, AstToken as _, SourceFile, SyntaxNode, WalkEvent, ast};

use super::{
    BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExprExpansion,
    ExpandedBodyMacro,
};

impl BodyMacroExpander<'_, '_> {
    /// Return each original template span once, even if a wrapper copies it into several calls.
    /// Skip literals supplied by the macro definition: they were not written in the call.
    pub fn format_string_spans(
        &mut self,
        package: &Package,
        crate_ref: CrateRef,
        file: FileId,
        syntax: &SourceFile,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Span>> {
        let Some(data) = self.def_maps.crate_data(crate_ref)? else {
            return Ok(Vec::new());
        };
        let Some(target) = package.target(data.cargo_target()) else {
            return Ok(Vec::new());
        };
        let cfg = CfgEvaluator::new(package.cfg_options(), target.enables_test_cfg());
        let mut spans = Vec::new();
        for call in self.format_macro_calls(syntax.syntax(), cfg)? {
            let span = Span::from_text_range(call.syntax().text_range());
            if span.intersection(range.unwrap_or(span)).is_none() {
                continue;
            }
            // We only highlight literals copied from this input. If none contains placeholder
            // parts, expansion cannot produce anything to highlight: `tracing::info!("ready")`
            // can stop here. This is only a filter; expansion still has to prove that a literal
            // is used for formatting. Checking macro names would miss user-defined wrappers.
            if !call.token_tree().is_some_and(|input| {
                input
                    .syntax()
                    .descendants_with_tokens()
                    .filter_map(|element| element.into_token())
                    .filter_map(ast::String::cast)
                    .any(|string| {
                        let mut has_parts = false;
                        string.format_parts(|_| has_parts = true);
                        has_parts
                    })
            }) {
                continue;
            }
            // Use the containing module's imports, so an alias such as
            // `use std::format_args as render` resolves to the actual formatting builtin.
            let path = enclosing_inline_module_path(call.syntax());
            let Some(module) = self
                .def_maps
                .module_for_inline_path(crate_ref, file, &path)?
            else {
                continue;
            };
            let site = BodyMacroCallSite::new(
                module,
                BodySource::written(file, span),
                BodyMacroCallOrigin::Source,
                package.edition(),
            )
            .with_cfg(cfg);
            self.collect_format_call(site, &call, &mut spans)?;
        }
        // One argument can be copied into several formatting calls by a wrapper.
        spans.sort_unstable_by_key(|span| (span.start, span.end));
        spans.dedup();
        Ok(spans)
    }

    /// Follow one call until expansion reaches a builtin that identifies its format template.
    fn collect_format_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
        spans: &mut Vec<Span>,
    ) -> anyhow::Result<()> {
        rg_std::check_cancel!(self.cancellation, "format macro expansion");
        let Some(_scope) = self.expansion_scope() else {
            return Ok(());
        };
        // `emit!("{user}");` can produce two `let` statements. Trying to parse that as one
        // expression would lose the expansion. Try statements first, as body lowering does;
        // expression builtins such as `format_args!` are handled by the fallback below.
        let statement = call
            .syntax()
            .parent()
            .filter(|node| ast::MacroExpr::can_cast(node.kind()))
            .and_then(|node| node.parent())
            .is_some_and(|node| ast::ExprStmt::can_cast(node.kind()));
        if statement
            && let Some(expanded) = self
                .expand_stmt_call(site.clone(), call)?
                .and_then(|outcome| outcome.expansion)
        {
            return self.collect_expanded_format_strings(&site, expanded, spans);
        }
        let Some(expansion) = self
            .expand_expr_call(site.clone(), call)?
            .and_then(|outcome| outcome.expansion)
        else {
            return Ok(());
        };
        match expansion {
            BodyMacroExprExpansion::Expanded(expanded) => {
                self.collect_expanded_format_strings(&site, expanded, spans)?;
            }
            BodyMacroExprExpansion::Builtin {
                kind,
                arguments: Some(arguments),
            } => {
                let (syntax, context) = arguments.into_syntax_and_context();
                // `format_args!(template, ...)` starts with the template, whereas
                // `assert!(condition, template, ...)` starts with the condition.
                let template = match kind {
                    BuiltinMacroExprKind::FormatArgs | BuiltinMacroExprKind::FormatArgsNl => {
                        syntax.template()
                    }
                    BuiltinMacroExprKind::Assert => syntax.args().next().and_then(|arg| arg.expr()),
                    _ => None,
                };
                // The expanded literal's offsets belong to generated text. Only keep it if
                // the token map proves it was copied unchanged from the original call.
                if let Some(ast::Expr::Literal(literal)) = template
                    && matches!(literal.kind(), ast::LiteralKind::String(_))
                    && let Some(span) =
                        context.source_for_exact_syntax(site.source(), literal.syntax())
                {
                    spans.push(span);
                }
                // In `format_args!("{}", format_args!("{user}"))`, the inner template needs
                // highlighting too. Its tokens must pass through this argument map as well.
                let nested = site.in_expansion(&context);
                for call in self.format_macro_calls(
                    syntax.syntax(),
                    site.cfg().expect("format query has cfg"),
                )? {
                    self.collect_format_call(nested.clone(), &call, spans)?;
                }
            }
            BodyMacroExprExpansion::Builtin {
                arguments: None, ..
            } => {}
        }
        Ok(())
    }

    /// Search a wrapper's output, carrying its token origins into each nested call.
    fn collect_expanded_format_strings<T: AstNode>(
        &mut self,
        site: &BodyMacroCallSite<'_>,
        expanded: ExpandedBodyMacro<T>,
        spans: &mut Vec<Span>,
    ) -> anyhow::Result<()> {
        let nested = site.in_expansion(&expanded);
        let (syntax, _) = expanded.into_syntax_and_context();
        for call in
            self.format_macro_calls(syntax.syntax(), site.cfg().expect("format query has cfg"))?
        {
            self.collect_format_call(nested.clone(), &call, spans)?;
        }
        Ok(())
    }

    /// Skip inactive syntax before expanding calls, including cfg gates produced by macros.
    fn format_macro_calls(
        &self,
        syntax: &SyntaxNode,
        cfg: CfgEvaluator<'_>,
    ) -> anyhow::Result<Vec<ast::MacroCall>> {
        let mut calls = Vec::new();
        let mut walk = syntax.preorder();
        let mut visited = 0usize;
        while let Some(event) = walk.next() {
            if visited.is_multiple_of(64) {
                rg_std::check_cancel!(self.cancellation, "format macro syntax");
            }
            visited += 1;
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            if ast::AnyHasAttrs::cast(node.clone())
                .is_some_and(|attrs| !cfg.is_syntax_enabled(&attrs))
            {
                walk.skip_subtree();
            } else if let Some(call) = ast::MacroCall::cast(node) {
                calls.push(call);
                // The outer macro decides what its input means. For example,
                // `discard!(format_args!("{user}"))` may discard the inner call altogether.
                // Visit nested calls only if they appear in the outer macro's expansion.
                walk.skip_subtree();
            }
        }
        Ok(calls)
    }
}
