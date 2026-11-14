//! Shared lowering context for expression bodies.

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{BodyMacroCallOrigin, BodyMacroExprExpansion, ExpandedBodyMacro};
use rg_ir_model::{CrateRef, ExprId, LocalDefRef, ModuleRef, ScopeId, SourceId, Span};
use rg_item_tree::{RecordSource, SourceRole, initializer_preview};
use rg_std::CancellationToken;
use rg_syntax::{AstNode, SyntaxNode, TextRange, ast};

use super::{
    CurrentDeclarationBuilder, CurrentRootItems,
    builder::{BodyBuilder, LoweredBodyData},
    macro_expansion::BodyMacroExpansionContext,
    syntax::source_for,
};
use crate::body::{BodyLocation, BodyMacroCallData, BodyOwner, BodySource, ExprData, ExprKind};

pub(super) struct BodyLowering<'a> {
    pub(super) owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    body_source: BodySource,
    pub(super) cfg: CfgEvaluator<'a>,
    pub(super) builder: BodyBuilder,
    pub(super) macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    generated_context: Option<GeneratedBodyMacroContext>,
    cancellation: &'a CancellationToken,
}

impl<'a> BodyLowering<'a> {
    pub(super) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        body_source: BodySource,
        cfg: CfgEvaluator<'a>,
        macro_expansion: &'a mut dyn BodyMacroExpansionContext,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            body_source,
            cfg,
            builder: BodyBuilder::default(),
            macro_expansion,
            generated_context: None,
            cancellation,
        }
    }

    fn lower_current_declaration(
        &mut self,
        item: ast::Item,
        role: CurrentRootItems,
        scope: ScopeId,
    ) -> anyhow::Result<()> {
        let declaration = CurrentDeclarationBuilder {
            file: self.body_source.file_id,
            cfg: self.cfg,
            items: &mut self.builder.source_items,
            sources: &mut self.builder.sources,
            cancellation: self.cancellation,
        }
        .root(item, role)
        .context("collect current body declarations")?;
        if let Some(declaration) = declaration {
            self.builder.scopes[scope].source_items.push(declaration);
        }
        Ok(())
    }

    pub(super) fn lower_function(
        mut self,
        function: ast::Fn,
        body: ast::BlockExpr,
        current_root_items: CurrentRootItems,
    ) -> anyhow::Result<LoweredBodyData> {
        // Parameters live in the function's outer lexical scope. The body block gets a child scope
        // so locals do not appear before the function boundary.
        let param_scope = self.builder.alloc_scope(None);
        self.lower_current_declaration(
            ast::Item::Fn(function.clone()),
            current_root_items,
            param_scope,
        )
        .context("lower function declarations")?;
        let function_params = self.lower_params(function.param_list(), param_scope);
        let root_expr = self.lower_block_expr(body, param_scope);

        Ok(self.builder.finish(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            param_scope,
            root_expr,
            function_params,
        ))
    }

    pub(super) fn lower_const(
        mut self,
        konst: ast::Const,
        expr: ast::Expr,
        current_root_items: CurrentRootItems,
    ) -> anyhow::Result<LoweredBodyData> {
        let root_scope = self.builder.alloc_scope(None);
        self.lower_current_declaration(
            ast::Item::Const(konst.clone()),
            current_root_items,
            root_scope,
        )
        .context("lower initializer declarations")?;
        Ok(self.lower_initializer(expr, root_scope))
    }

    pub(super) fn lower_static(
        mut self,
        static_: ast::Static,
        expr: ast::Expr,
        current_root_items: CurrentRootItems,
    ) -> anyhow::Result<LoweredBodyData> {
        let root_scope = self.builder.alloc_scope(None);
        self.lower_current_declaration(
            ast::Item::Static(static_.clone()),
            current_root_items,
            root_scope,
        )
        .context("lower initializer declarations")?;
        Ok(self.lower_initializer(expr, root_scope))
    }

    fn lower_initializer(mut self, expr: ast::Expr, root_scope: ScopeId) -> LoweredBodyData {
        // Item initializers are expression bodies without parameters. They still need a root scope
        // so ordinary body path resolution, type paths, and source scans can use the same pipeline
        // as function bodies.
        let preview = initializer_preview(&expr);
        let root_expr = self.lower_expr(expr, root_scope);
        // A current initializer can reuse a saved declaration. Its display text must still come
        // from this body snapshot, even when the current expression is too long for a preview.
        self.builder
            .sources
            .record_initializer_preview(self.builder.exprs[root_expr].source, preview);

        self.builder.finish(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            root_scope,
            root_expr,
            Vec::new(),
        )
    }
}

impl BodyLowering<'_> {
    pub(super) fn record_unmodeled_syntax(&mut self, syntax: &SyntaxNode) {
        self.builder
            .unmodeled_syntax
            .push(syntax.text().to_string());
    }

    pub(super) fn alloc_expr(
        &mut self,
        syntax: &SyntaxNode,
        scope: ScopeId,
        kind: ExprKind,
    ) -> ExprId {
        self.alloc_expr_with_source(self.source(syntax), scope, kind)
    }

    pub(super) fn alloc_expr_with_source(
        &mut self,
        source: BodySource,
        scope: ScopeId,
        kind: ExprKind,
    ) -> ExprId {
        // Name resolution uses this boundary to avoid seeing bindings introduced after the
        // expression, while still allowing earlier bindings in the same lexical scope.
        let visible_bindings = self.builder.bindings.len();
        let source = self.builder.sources.record(source);
        self.builder.alloc_expr(ExprData {
            source,
            scope,
            visible_bindings,
            kind,
        })
    }

    pub(super) fn source(&self, syntax: &SyntaxNode) -> BodySource {
        if let Some(context) = &self.generated_context {
            // Macro expansion provenance is intentionally token-sized. A larger generated node can
            // contain substituted input while still being structurally created by the transcriber.
            if let Some(span) = context
                .expanded
                .source_for_exact_syntax(context.source, syntax)
            {
                return BodySource::macro_generated(context.source.file_id, span);
            }
            return BodySource::macro_generated(context.source.file_id, context.source.span);
        }

        source_for(self.body_source.file_id, syntax)
    }

    /// Translate a range within one expansion token back to the original macro argument.
    ///
    /// Path segments and member names need the same copied-token check as expression sources;
    /// template tokens get the whole call span as a non-editable navigation fallback.
    pub(super) fn source_location(&mut self, range: TextRange) -> SourceId {
        self.with_sources(|sources| sources.record(range))
    }

    pub(super) fn source_id(&mut self, syntax: &SyntaxNode) -> SourceId {
        let source = self.source(syntax);
        self.builder.sources.record(source)
    }

    /// Keep an occurrence only when its bytes belong to written source. Generated template text
    /// can have a navigation fallback, but that fallback cannot stand in for a format name.
    pub(super) fn written_source_span(&self, range: TextRange) -> Option<Span> {
        match &self.generated_context {
            Some(context) => context.expanded.source_for_range(context.source, range),
            None => Some(Span::from_text_range(range)),
        }
    }

    /// Record the physical interpretation while syntax and macro provenance are available.
    /// Type and declaration lowering receive the same recorder in ordinary and generated bodies.
    ///
    /// All calls append to this body's occurrence sequence, including type annotations that do
    /// not declare an item. Locations are resolved here; reading them does not need to replay
    /// token origins later.
    pub(super) fn with_sources<T>(&mut self, lower: impl FnOnce(&mut dyn RecordSource) -> T) -> T {
        let context = self.generated_context.as_ref();
        let map = |range, role| match context {
            Some(context) => {
                let copied = matches!(role, SourceRole::Syntax)
                    .then(|| context.expanded.source_for_range(context.source, range))
                    .flatten();
                BodyLocation {
                    source: BodySource::macro_generated(
                        context.source.file_id,
                        copied.unwrap_or(context.source.span),
                    ),
                    editable: copied.is_some(),
                }
            }
            None => BodyLocation {
                source: BodySource::written(self.body_source.file_id, Span::from_text_range(range)),
                editable: true,
            },
        };
        lower(&mut self.builder.sources.recorder(&map))
    }

    pub(super) fn dollar_crate(&self) -> Option<CrateRef> {
        self.generated_context
            .as_ref()
            .map(|context| context.expanded.dollar_crate())
    }

    /// Classify the macro call syntax that is about to be expanded.
    ///
    /// User-written calls have no macro-definition crate. Calls found while recursively lowering
    /// generated syntax keep the generating definition's `$crate` identity and token map.
    pub(super) fn macro_call_origin(&self) -> BodyMacroCallOrigin {
        match &self.generated_context {
            Some(context) => BodyMacroCallOrigin::Generated {
                dollar_crate: context.expanded.dollar_crate(),
                source_map: context.expanded.source_map(),
                input_sources: context.expanded.input_sources(),
                call_site: context.expanded.call_site(),
            },
            None => BodyMacroCallOrigin::Source,
        }
    }

    /// Pick the semantic module used to resolve a body macro call.
    ///
    /// Top-level bodies resolve in their owner module. Body-local generated/nested bodies may have
    /// a synthetic module owner, so they fall back to the surrounding semantic module.
    ///
    /// Example: a macro inside a normal function resolves from that function's module, while a
    /// macro inside a generated nested body still resolves from the nearest real module.
    pub(super) fn macro_resolution_module(&self) -> ModuleRef {
        if self.owner_module.origin.as_crate_ref().is_some() {
            self.owner_module
        } else {
            self.fallback_module
        }
    }

    /// Lower one expanded macro under its source-map and macro-definition crate context.
    pub(super) fn with_expanded_macro<Syntax: AstNode, Output>(
        &mut self,
        call_source: BodySource,
        expanded: ExpandedBodyMacro<Syntax>,
        f: impl FnOnce(&mut Self, Syntax) -> Output,
    ) -> Output {
        // Save the proven input spans in Body IR while the full expansion tree is still available.
        // The tree and its token map only need to live through recursive lowering.
        self.builder
            .sources
            .provenance
            .record_expansion_arguments(expanded.argument_spans(call_source));
        let (syntax, expanded_context) = expanded.into_syntax_and_context();
        let context = GeneratedBodyMacroContext::new(call_source, expanded_context);
        // A nested expansion uses its own map, then restores the parent before lowering continues.
        let previous = self.generated_context.replace(context);
        let result = f(self, syntax);
        self.generated_context = previous;
        result
    }

    /// Record a user-written macro call as a navigation/reference candidate.
    ///
    /// A nested invocation copied from an argument is still user-written syntax, while helper
    /// invocations introduced by the template stay hidden. Keep the definition selected by
    /// expansion so source queries use the same macro as lowering.
    pub(super) fn record_source_macro_call(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        origin: BodyMacroCallOrigin,
        definition: LocalDefRef,
    ) {
        let Some(path) = call.path() else {
            return;
        };
        let Some(name_ref) = path.segment().and_then(|segment| segment.name_ref()) else {
            return;
        };

        if !matches!(origin, BodyMacroCallOrigin::Source)
            && self
                .generated_context
                .as_ref()
                .and_then(|context| {
                    context
                        .expanded
                        .source_for_exact_syntax(context.source, name_ref.syntax())
                })
                .is_none()
        {
            return;
        }

        let source = self.builder.sources.record(call_source);
        let name_location = self.source_location(name_ref.syntax().text_range());
        self.builder.push_macro_call(BodyMacroCallData {
            source,
            name_location,
            definition,
        });
    }

    /// Resolve an expression-position macro call from its original source location.
    ///
    /// Example: `let value = make_expr!(input);` expands to syntax and is lowered in the current
    /// lexical scope. `format_args!("hi")` becomes a builtin expression only if no user macro with
    /// that name resolves first. If both paths fail, the caller keeps the original macro
    /// expression.
    pub(super) fn lower_macro_call_from_call_site(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        scope: ScopeId,
    ) -> Option<ExprId> {
        let module = self.macro_resolution_module();
        let origin = self.macro_call_origin();

        let _expansion_scope = self.macro_expansion.expansion_scope()?;

        // Expansion is best-effort during mechanical lowering: unresolved, unsupported, or failing
        // macros should leave normal unknown expressions instead of aborting the whole body build.
        let outcome = self
            .macro_expansion
            .expand_expr_call(module, call_source, origin.clone(), call)
            .ok()
            .flatten()?;
        self.record_source_macro_call(call_source, call, origin, outcome.definition);
        let expansion = outcome.expansion?;

        match expansion {
            BodyMacroExprExpansion::Expanded(expanded) => {
                // The macro call remains the fallback range for generated syntax. Leaf syntax that
                // the expansion span map can trace back to invocation input gets a narrower source
                // span during recursive lowering.
                Some(
                    self.with_expanded_macro(call_source, expanded, |this, syntax| {
                        this.lower_expr(syntax, scope)
                    }),
                )
            }
            BodyMacroExprExpansion::Builtin { kind, arguments } => {
                // Builtin input is parsed separately, so lower it with its own token map just as
                // we lower declarative output. The surrounding builtin keeps its intrinsic type.
                let args = arguments
                    .map(|arguments| {
                        self.with_expanded_macro(call_source, arguments, |this, syntax| {
                            this.lower_builtin_macro_args(kind, syntax, scope)
                        })
                    })
                    .unwrap_or_default();
                Some(self.alloc_expr_with_source(
                    call_source,
                    scope,
                    ExprKind::BuiltinMacro { kind, args },
                ))
            }
        }
    }
}

/// Temporary context for syntax produced by one body macro expansion.
///
/// The macro call source is the stable fallback for generated syntax. When the expansion span map
/// proves that a small token came from the invocation, the token gets its original argument span
/// instead. For example, `make_expr!(input)` can expose `input` as the source of a generated path,
/// while the surrounding binary expression still belongs to the macro call.
struct GeneratedBodyMacroContext {
    source: BodySource,
    expanded: ExpandedBodyMacro<SyntaxNode>,
}

impl GeneratedBodyMacroContext {
    fn new(source: BodySource, expanded: ExpandedBodyMacro<SyntaxNode>) -> Self {
        Self { source, expanded }
    }
}
