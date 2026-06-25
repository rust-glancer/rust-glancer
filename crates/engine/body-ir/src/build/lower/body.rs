//! Shared lowering context for expression bodies.

use rg_syntax::{AstNode as _, ast};

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{BodyMacroCallOrigin, BodyMacroExprExpansion, ExpandedBodyMacro};
use rg_ir_model::{BodyMacroCallData, ExprId, LocalDefRef, ModuleRef, ScopeId, TargetRef};
use rg_parse::LineIndex;
use rg_text::NameInterner;

use crate::ir::{BodyBuilder, BodyOwner, BodySource, ExprData, ExprKind, ResolvedBodyData};

use super::{macro_expansion::BodyMacroExpansionContext, syntax::source_for};

pub(super) struct BodyLowering<'a> {
    owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    body_source: BodySource,
    pub(super) cfg: CfgEvaluator<'a>,
    pub(super) line_index: &'a LineIndex,
    pub(super) interner: &'a mut NameInterner,
    pub(super) builder: BodyBuilder,
    pub(super) macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    generated_context: Option<GeneratedBodyMacroContext>,
}

/// Temporary context for syntax produced by one body macro expansion.
///
/// The macro call source is the stable fallback for generated syntax. When the expansion span map
/// proves that a small token came from the invocation, the token gets its original argument span
/// instead. For example, `make_expr!(input)` can expose `input` as the source of a generated path,
/// while the surrounding binary expression still belongs to the macro call.
struct GeneratedBodyMacroContext {
    source: BodySource,
    expanded: ExpandedBodyMacro<rg_syntax::SyntaxNode>,
}

impl GeneratedBodyMacroContext {
    fn new(source: BodySource, expanded: ExpandedBodyMacro<rg_syntax::SyntaxNode>) -> Self {
        Self { source, expanded }
    }
}

impl<'a> BodyLowering<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        body_source: BodySource,
        cfg: CfgEvaluator<'a>,
        line_index: &'a LineIndex,
        interner: &'a mut NameInterner,
        macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            body_source,
            cfg,
            line_index,
            interner,
            builder: BodyBuilder::default(),
            macro_expansion,
            generated_context: None,
        }
    }

    pub(super) fn lower_function(
        mut self,
        function: ast::Fn,
        body: ast::BlockExpr,
    ) -> ResolvedBodyData {
        // Parameters live in the function's outer lexical scope. The body block gets a child scope
        // so locals do not appear before the function boundary.
        let param_scope = self.builder.alloc_scope(None);
        let function_params = self.lower_params(function.param_list(), param_scope);
        let params = function_params
            .iter()
            .flat_map(|param| param.bindings.iter().copied())
            .collect();
        let root_expr = self.lower_block_expr(body, param_scope);

        ResolvedBodyData::new(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            param_scope,
            root_expr,
            function_params,
            params,
            self.builder,
        )
    }

    pub(super) fn lower_initializer(mut self, expr: ast::Expr) -> ResolvedBodyData {
        // Item initializers are expression bodies without parameters. They still need a root scope
        // so ordinary body path resolution, type paths, and source scans can use the same pipeline
        // as function bodies.
        let root_scope = self.builder.alloc_scope(None);
        let root_expr = self.lower_expr(expr, root_scope);

        ResolvedBodyData::new(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            root_scope,
            root_expr,
            Vec::new(),
            Vec::new(),
            self.builder,
        )
    }
}

impl BodyLowering<'_> {
    pub(super) fn alloc_expr(
        &mut self,
        syntax: &rg_syntax::SyntaxNode,
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
        self.builder.alloc_expr(ExprData {
            source,
            scope,
            visible_bindings,
            kind,
        })
    }

    pub(super) fn source(&self, syntax: &rg_syntax::SyntaxNode) -> BodySource {
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

    pub(super) fn dollar_crate_target(&self) -> Option<TargetRef> {
        self.generated_context
            .as_ref()
            .map(|context| context.expanded.dollar_crate_target())
    }

    /// Classify the macro call syntax that is about to be expanded.
    ///
    /// User-written calls have no macro-definition crate. Calls found while recursively lowering
    /// generated syntax keep the generating definition's `$crate` target.
    pub(super) fn macro_call_origin(&self) -> BodyMacroCallOrigin {
        match &self.generated_context {
            Some(context) => BodyMacroCallOrigin::Generated {
                dollar_crate_target: context.expanded.dollar_crate_target(),
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
        if self.owner_module.origin.as_target_ref().is_some() {
            self.owner_module
        } else {
            self.fallback_module
        }
    }

    /// Lower one expanded macro under its source-map and macro-definition crate context.
    pub(super) fn with_expanded_macro<Syntax: rg_syntax::AstNode, Output>(
        &mut self,
        call_source: BodySource,
        expanded: ExpandedBodyMacro<Syntax>,
        f: impl FnOnce(&mut Self, Syntax) -> Output,
    ) -> Output {
        let (syntax, expanded_context) = expanded.into_syntax_and_context();
        let context = GeneratedBodyMacroContext::new(call_source, expanded_context);
        let previous = self.generated_context.replace(context);
        let result = f(self, syntax);
        self.generated_context = previous;
        result
    }

    /// Record a user-written macro call as a navigation/reference candidate.
    ///
    /// Generated nested macro calls are implementation details of expansion, so only source calls
    /// are indexed. The definition comes from the expansion outcome, which keeps hover/goto
    /// aligned with the macro that lowering actually selected.
    pub(super) fn record_source_macro_call(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        origin: BodyMacroCallOrigin,
        definition: LocalDefRef,
    ) {
        if !matches!(origin, BodyMacroCallOrigin::Source) {
            return;
        }

        let Some(path) = call.path() else {
            return;
        };
        let Some(name_ref) = path.segment().and_then(|segment| segment.name_ref()) else {
            return;
        };

        self.builder.push_macro_call(BodyMacroCallData {
            source: call_source,
            name_span: self.source(name_ref.syntax()).span,
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
            .expand_expr_call(module, call_source, origin, call)
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
            BodyMacroExprExpansion::Builtin(kind) => Some(self.alloc_expr_with_source(
                call_source,
                scope,
                ExprKind::BuiltinMacro { kind },
            )),
        }
    }
}
