//! Shared lowering context for expression bodies.

use rg_syntax::ast;

use rg_def_map::ExpandedBodyMacro;
use rg_ir_model::{ExprId, ModuleRef, ScopeId, TargetRef};
use rg_parse::LineIndex;
use rg_text::NameInterner;

use crate::ir::{BodyBuilder, BodyOwner, BodySource, ExprData, ExprKind, ResolvedBodyData};

use super::{macro_expansion::BodyMacroExpansionContext, syntax::source_for};

pub(super) struct BodyLowering<'a> {
    owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    body_source: BodySource,
    pub(super) line_index: &'a LineIndex,
    pub(super) interner: &'a mut NameInterner,
    pub(super) builder: BodyBuilder,
    pub(super) macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    generated_context: Option<GeneratedBodyMacroContext>,
}

/// Temporary context for syntax produced by one body macro expansion.
#[derive(Clone, Copy)]
struct GeneratedBodyMacroContext {
    source: BodySource,
    dollar_crate_target: TargetRef,
}

impl GeneratedBodyMacroContext {
    fn new(source: BodySource, dollar_crate_target: TargetRef) -> Self {
        Self {
            source,
            dollar_crate_target,
        }
    }
}

impl<'a> BodyLowering<'a> {
    pub(super) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        body_source: BodySource,
        line_index: &'a LineIndex,
        interner: &'a mut NameInterner,
        macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            body_source,
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
        // Name resolution uses this boundary to avoid seeing bindings introduced after the
        // expression, while still allowing earlier bindings in the same lexical scope.
        let visible_bindings = self.builder.bindings.len();
        self.builder.alloc_expr(ExprData {
            source: self.source(syntax),
            scope,
            visible_bindings,
            kind,
        })
    }

    pub(super) fn source(&self, syntax: &rg_syntax::SyntaxNode) -> BodySource {
        if let Some(context) = self.generated_context {
            return context.source;
        }

        source_for(self.body_source.file_id, syntax)
    }

    pub(super) fn dollar_crate_target(&self) -> Option<TargetRef> {
        self.generated_context
            .map(|context| context.dollar_crate_target)
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

    /// Lower one expanded macro under the call-site source and macro-definition crate context.
    pub(super) fn with_expanded_macro<Syntax, Output>(
        &mut self,
        call_source: BodySource,
        expanded: ExpandedBodyMacro<Syntax>,
        f: impl FnOnce(&mut Self, Syntax) -> Output,
    ) -> Output {
        let context = GeneratedBodyMacroContext::new(call_source, expanded.dollar_crate_target());
        let syntax = expanded.into_syntax();
        let previous = self.generated_context.replace(context);
        let result = f(self, syntax);
        self.generated_context = previous;
        result
    }

    /// Expand and lower an expression-position macro call from its original source location.
    ///
    /// Example: `let value = make_expr!(input);` expands to an expression and is lowered in the
    /// current lexical scope. If expansion fails, the caller keeps the original macro expression.
    pub(super) fn lower_macro_call_from_call_site(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        scope: ScopeId,
    ) -> Option<ExprId> {
        let _expansion_scope = self.macro_expansion.expansion_scope()?;
        let module = self.macro_resolution_module();
        let target = module.origin.origin_target();

        // Expansion is best-effort during mechanical lowering: an unresolved or failing macro
        // should leave a normal unknown expression instead of aborting the whole body build.
        let expanded = self
            .macro_expansion
            .expand_expr_call(target, module, call_source.file_id, call_source.span, call)
            .ok()
            .flatten()?;

        // Generated body syntax carries no fine-grained expansion span map at this boundary, so
        // every lowered node in the expansion is anchored to the macro call site.
        Some(
            self.with_expanded_macro(call_source, expanded, |this, syntax| {
                this.lower_expr(syntax, scope)
            }),
        )
    }
}
