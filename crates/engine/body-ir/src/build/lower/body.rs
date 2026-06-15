//! Shared lowering context for expression bodies.

use rg_syntax::ast;

use rg_ir_model::{ExprId, ModuleRef, ScopeId};
use rg_parse::LineIndex;
use rg_text::NameInterner;

use crate::ir::{BodyBuilder, BodyOwner, BodySource, ExprData, ExprKind, ResolvedBodyData};

use super::syntax::source_for;

pub(super) struct BodyLowering<'a> {
    owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    body_source: BodySource,
    pub(super) line_index: &'a LineIndex,
    pub(super) interner: &'a mut NameInterner,
    pub(super) builder: BodyBuilder,
}

impl<'a> BodyLowering<'a> {
    pub(super) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        body_source: BodySource,
        line_index: &'a LineIndex,
        interner: &'a mut NameInterner,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            body_source,
            line_index,
            interner,
            builder: BodyBuilder::default(),
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
        source_for(self.body_source.file_id, syntax)
    }
}
