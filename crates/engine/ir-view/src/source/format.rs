//! Find caller-written format strings by following macro expansions.
//!
//! A call such as `render!("{user}")` may forward its arguments through several wrappers before
//! reaching `format_args!`. Follow those expansions to identify the format template, then use
//! the token maps to recover the literal's position in the original file.
//!
//! Highlighting only needs that position and the placeholder syntax. Preparing analyzed function
//! bodies would also resolve names and infer types, which this query does not need.

use rg_def_map::{
    BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExprExpansion,
    ExpandedBodyMacro,
};
use rg_ir_model::{BodySource, CrateRef, FileId, Span};
use rg_parse::enclosing_inline_module_path;
use rg_syntax::{AstNode, AstToken as _, SourceFile, SyntaxNode, WalkEvent, ast};
use rg_workspace::CrateConfig;

use crate::IndexedViewDb;

pub struct FormatStringSourceView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> FormatStringSourceView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return the written template spans from calls that touch the requested range. A wrapper
    /// may copy the same literal into several formatting calls, so return its source span once.
    /// A template supplied by the macro definition has no span to highlight at the call site.
    pub fn spans(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        syntax: &SourceFile,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Span>> {
        let config = self.db.crate_config(crate_ref)?;
        let mut scan = FormatStringScan {
            db: self.db,
            config: &config,
            expander: BodyMacroExpander::new(&self.db.def_map),
            spans: Vec::new(),
        };
        scan.scan_file(crate_ref, file, syntax, range)?;
        // One argument can be copied into several formatting calls by a wrapper.
        scan.spans
            .sort_unstable_by_key(|span| (span.start, span.end));
        scan.spans.dedup();
        Ok(scan.spans)
    }
}

/// One scan shares the expansion cache and recursion limit across the file and its nested calls.
struct FormatStringScan<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    config: &'a CrateConfig,
    expander: BodyMacroExpander<'db, 'a>,
    spans: Vec<Span>,
}

impl FormatStringScan<'_, '_> {
    fn scan_file(
        &mut self,
        crate_ref: CrateRef,
        file: FileId,
        syntax: &SourceFile,
        range: Option<Span>,
    ) -> anyhow::Result<()> {
        let config = self.config;
        let cfg = config.cfg();
        for call in self.format_macro_calls(syntax.syntax())? {
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
                .db
                .def_map
                .module_for_inline_path(crate_ref, file, &path)?
            else {
                continue;
            };
            let site = BodyMacroCallSite::new(
                module,
                BodySource::written(file, span),
                BodyMacroCallOrigin::Source,
                config.edition,
            )
            .with_cfg(cfg);
            self.collect_format_call(site, &call)?;
        }
        Ok(())
    }

    /// Follow one call until expansion reaches a builtin that identifies its format template.
    fn collect_format_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<()> {
        rg_std::check_cancel!(self.db, "format macro expansion");
        let Some(_scope) = self.expander.expansion_scope() else {
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
                .expander
                .expand_stmt_call(site.clone(), call)?
                .and_then(|outcome| outcome.expansion)
        {
            return self.collect_expanded_format_strings(&site, expanded);
        }
        let Some(expansion) = self
            .expander
            .expand_expr_call(site.clone(), call)?
            .and_then(|outcome| outcome.expansion)
        else {
            return Ok(());
        };
        match expansion {
            BodyMacroExprExpansion::Expanded(expanded) => {
                self.collect_expanded_format_strings(&site, expanded)?;
            }
            BodyMacroExprExpansion::Builtin {
                arguments: Some(arguments),
                format_template,
                ..
            } => {
                let (syntax, context) = arguments.into_syntax_and_context();
                // The expanded literal's offsets belong to generated text. Only keep it if
                // the token map proves it was copied unchanged from the original call.
                if let Some(ast::Expr::Literal(literal)) = format_template
                    && matches!(literal.kind(), ast::LiteralKind::String(_))
                    && let Some(span) =
                        context.source_for_exact_syntax(site.source(), literal.syntax())
                {
                    self.spans.push(span);
                }
                // In `format_args!("{}", format_args!("{user}"))`, the inner template needs
                // highlighting too. Its tokens must pass through this argument map as well.
                let nested = site.in_expansion(&context);
                for call in self.format_macro_calls(syntax.syntax())? {
                    self.collect_format_call(nested.clone(), &call)?;
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
    ) -> anyhow::Result<()> {
        let nested = site.in_expansion(&expanded);
        let (syntax, _) = expanded.into_syntax_and_context();
        for call in self.format_macro_calls(syntax.syntax())? {
            self.collect_format_call(nested.clone(), &call)?;
        }
        Ok(())
    }

    /// Skip inactive syntax before expanding calls, including cfg gates produced by macros.
    fn format_macro_calls(&self, syntax: &SyntaxNode) -> anyhow::Result<Vec<ast::MacroCall>> {
        let cfg = self.config.cfg();
        let mut calls = Vec::new();
        let mut walk = syntax.preorder();
        let mut visited = 0usize;
        while let Some(event) = walk.next() {
            if visited.is_multiple_of(64) {
                rg_std::check_cancel!(self.db, "format macro syntax");
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
