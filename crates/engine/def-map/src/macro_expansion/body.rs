//! Body-facing declarative macro expansion.
//!
//! Body lowering needs expansion as an input to syntax lowering, but it should not know about the
//! token-tree and macro-engine crates directly. This facade keeps body-specific frozen def-map
//! visibility and token conversion next to the def-map query it relies on.

use anyhow::Context as _;

use rg_ir_model::{BodySource, DefMapRef, LocalDefRef, ModuleId, ModuleRef, TargetRef};
use rg_ir_storage::{
    DefMapQuery, ImportPath, MacroDefinitionData, MacroDefinitionView, PathResolver,
    ScopeResolutionEnv,
};
use rg_macro_runtime::{
    ExpansionParseKind, ExpansionSyntax, MacroExpansionRequest, MacroExpansionRuntime,
    macro_edition,
};
use rg_parse::{FileId, Span};
use rg_std::ExpectedUnique;
use rg_syntax::{AstNode, Parse, SyntaxNode, ast, utils::normalized_syntax_text};
use rg_text::Name;
use rg_tt::TopSubtree;
use rg_tt::syntax_bridge::{ExpansionSpanMap, SpanFactory, syntax_node_to_token_tree_with_span};
use rg_workspace::RustEdition;

use crate::DefMapReadTxn;

/// Generated body syntax plus the macro-definition origin needed while lowering it.
pub struct ExpandedBodyMacro<T> {
    syntax: T,
    source_map: ExpansionSpanMap,
    dollar_crate_target: TargetRef,
}

impl<T> ExpandedBodyMacro<T> {
    fn new(syntax: T, source_map: ExpansionSpanMap, dollar_crate_target: TargetRef) -> Self {
        Self {
            syntax,
            source_map,
            dollar_crate_target,
        }
    }

    /// Maps generated syntax back to a macro invocation token when that mapping is precise.
    pub fn source_for_exact_syntax(
        &self,
        call_source: BodySource,
        syntax: &SyntaxNode,
    ) -> Option<Span> {
        let range = syntax.text_range();

        // Only accept exact single-token source mappings from the original call. Compound
        // generated nodes often contain substituted tokens, but the node as a whole was shaped by
        // the macro transcriber and should stay anchored to the call site.
        let token = syntax.first_token()?;
        if token != syntax.last_token()? || token.text_range() != range {
            return None;
        }

        let span = self
            .source_map
            .span_for_range_in_file(range, call_source.file_id.0)?;
        let span = Span::from_text_range(span.range);
        (call_source.span.contains_span(span) && span.len() == u32::from(range.len()))
            .then_some(span)
    }

    /// Returns the crate that generated `$crate` paths inside this syntax should resolve to.
    pub fn dollar_crate_target(&self) -> TargetRef {
        self.dollar_crate_target
    }
}

impl<T: AstNode> ExpandedBodyMacro<T> {
    /// Splits out typed generated syntax while keeping source context active for descendants.
    pub fn into_syntax_and_context(self) -> (T, ExpandedBodyMacro<SyntaxNode>) {
        let Self {
            syntax,
            source_map,
            dollar_crate_target,
        } = self;
        let context_syntax = syntax.syntax().clone();
        (
            syntax,
            ExpandedBodyMacro::new(context_syntax, source_map, dollar_crate_target),
        )
    }
}

impl ExpandedBodyMacro<Parse<SyntaxNode>> {
    fn cast_root_or_child<N: AstNode>(self) -> Option<ExpandedBodyMacro<N>> {
        let Self {
            syntax: parse,
            source_map,
            dollar_crate_target,
        } = self;
        let root = parse.syntax_node();
        let syntax = N::cast(root.clone()).or_else(|| root.children().find_map(N::cast))?;
        Some(ExpandedBodyMacro::new(
            syntax,
            source_map,
            dollar_crate_target,
        ))
    }
}

/// Expands declarative macros for Body IR lowering using frozen def-map visibility.
pub struct BodyMacroExpander<'db, 'txn> {
    def_maps: &'txn DefMapReadTxn<'db>,
    runtime: MacroExpansionRuntime,
}

impl<'db, 'txn> BodyMacroExpander<'db, 'txn> {
    pub fn new(def_maps: &'txn DefMapReadTxn<'db>) -> Self {
        Self {
            def_maps,
            runtime: MacroExpansionRuntime::default(),
        }
    }

    /// Expands one expression-position macro call to generated expression syntax.
    pub fn expand_expr_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Expr>>> {
        let Some(expanded) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Expr,
        )?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::Expr>())
    }

    /// Expands one statement-position macro call to generated statement-list syntax.
    pub fn expand_stmt_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::MacroStmts>>> {
        let Some(expanded) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Statements,
        )?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::MacroStmts>())
    }

    /// Expands one pattern-position macro call to generated pattern syntax.
    pub fn expand_pat_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Pat>>> {
        let Some(expanded) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Pattern,
        )?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::Pat>())
    }

    /// Expands one type-position macro call to generated type syntax.
    pub fn expand_type_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Type>>> {
        let Some(expanded) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Type,
        )?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::Type>())
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_call_syntax(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
        parse_kind: ExpansionParseKind,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<Parse<SyntaxNode>>>> {
        let Some(invocation) =
            BodyMacroInvocation::from_ast(file_id, span, parse_package.edition(), call)
        else {
            return Ok(None);
        };
        // Note: generated body syntax carries its macro-definition crate through
        // `ExpandedBodyMacro`. The invocation path itself does not yet have that context, so a
        // body call written through `$crate::macro_name!()` stays unresolved.
        let Some(path) = ImportPath::from_macro_path_text(invocation.path_text(), None) else {
            return Ok(None);
        };

        let query = DefMapQuery::new(self.def_maps);
        let Some(resolved) = Self::resolve_macro_definition(&query, target, module, &path)
            .context("while attempting to resolve body macro call")?
        else {
            return Ok(None);
        };

        let request = invocation.expansion_request(resolved.def_ref, resolved.data, parse_kind);
        let Some(ExpansionSyntax { parse, span_map }) = self.runtime.expand_now(request) else {
            return Ok(None);
        };

        Ok(Some(ExpandedBodyMacro::new(
            parse,
            span_map,
            resolved.data.dollar_crate_target,
        )))
    }

    fn resolve_macro_definition<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleRef,
        path: &ImportPath,
    ) -> anyhow::Result<Option<MacroDefinitionView<'a>>> {
        // Body expansion is target-local. Synthetic body modules resolve through their semantic
        // fallback before reaching this facade.
        let Some(module_target) = module.origin.as_target_ref() else {
            return Ok(None);
        };
        if module_target != target {
            return Ok(None);
        }

        if let Some(name) = path.relative_single_name() {
            return Self::resolve_single_name_macro(query, target, module.module, name);
        }

        let bindings = PathResolver::new(query)
            .macro_bindings(target, module.module, path)
            .context("while attempting to resolve qualified body macro path")?;
        let mut macros = ExpectedUnique::new();
        for binding in bindings {
            // Path resolution may return duplicate bindings to the same macro definition; collapse
            // those while still treating different definitions as ambiguous.
            if let Some(macro_) = query
                .macro_definition_view(binding.def)
                .context("while attempting to fetch body macro definition")?
            {
                macros.push(macro_);
            }
        }

        Ok(macros.into_option())
    }

    fn resolve_single_name_macro<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleId,
        name: &Name,
    ) -> anyhow::Result<Option<MacroDefinitionView<'a>>> {
        let mut resolved = ExpectedUnique::new();
        let mut current = Some(module);

        // Note: Body macro expansion intentionally uses the frozen module graph as an approximation
        // of item-position macro visibility. We do not model body-specific textual ordering here:
        // doing so would require cross-body statement order and nested body macro scope tracking,
        // while real projects overwhelmingly use macros that are already module-visible.
        while let Some(module) = current {
            let module_ref = ModuleRef {
                origin: DefMapRef::Target(target),
                module,
            };
            if let Some(entry) = query
                .module_scope_entry(module_ref, name.as_str())
                .context("while attempting to inspect body macro scope entry")?
            {
                for binding in entry.macros() {
                    if let Some(macro_) = query
                        .macro_definition_view(binding.def)
                        .context("while attempting to fetch body macro definition")?
                    {
                        resolved.push(macro_);
                    }
                }
            }

            current = query
                .module_data(module_ref)
                .context("while attempting to fetch parent module for body macro lookup")?
                .and_then(|module| module.parent);
        }

        Ok(resolved.into_option())
    }
}

/// Body-specific adapter from parsed macro-call syntax to runtime expansion input.
///
/// Item-position calls are already lowered by item-tree before def-map expansion sees them. Bodies
/// arrive here as `ast::MacroCall`, so this private adapter keeps the AST and token-tree conversion
/// next to the body visibility policy instead of making `rg_macro_runtime` depend on parsed AST.
struct BodyMacroInvocation {
    path_text: String,
    args: TopSubtree,
    call_file_id: FileId,
    call_span: Span,
    call_edition: RustEdition,
}

impl BodyMacroInvocation {
    fn from_ast(
        file_id: FileId,
        span: Span,
        edition: RustEdition,
        call: &ast::MacroCall,
    ) -> Option<Self> {
        let path_text = call.path().map(|path| normalized_syntax_text(&path))?;
        let args = call.token_tree()?;

        let span_factory = SpanFactory::new(
            u32::try_from(file_id.0).expect("file id should fit macro span storage"),
            macro_edition(edition),
        );
        let args =
            syntax_node_to_token_tree_with_span(&args, &mut |range| span_factory.span_for(range));

        Some(Self {
            path_text,
            args,
            call_file_id: file_id,
            call_span: span,
            call_edition: edition,
        })
    }

    fn path_text(&self) -> &str {
        &self.path_text
    }

    fn expansion_request<'a>(
        &'a self,
        def_ref: LocalDefRef,
        definition: &'a MacroDefinitionData,
        parse_kind: ExpansionParseKind,
    ) -> MacroExpansionRequest<'a> {
        MacroExpansionRequest {
            def_ref,
            definition,
            path_text: &self.path_text,
            args: &self.args,
            call_file_id: self.call_file_id,
            call_span: self.call_span,
            call_edition: self.call_edition,
            parse_kind,
        }
    }
}
