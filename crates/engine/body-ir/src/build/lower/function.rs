//! Shared lowering context for one function body.

use rg_syntax::ast;

use rg_ir_model::{FunctionRef, ModuleRef};
use rg_parse::LineIndex;
use rg_text::NameInterner;
use rg_ty::IndexedTy;

use crate::ir::{
    BodyBuilder, BodyData, BodyResolution, BodySource, ExprData, ExprId, ExprKind, ScopeId,
};

use super::syntax::source_for;

pub(super) struct FunctionBodyLowering<'a> {
    owner: FunctionRef,
    owner_module: ModuleRef,
    function_source: BodySource,
    pub(super) line_index: &'a LineIndex,
    pub(super) interner: &'a mut NameInterner,
    pub(super) builder: BodyBuilder,
}

impl<'a> FunctionBodyLowering<'a> {
    pub(super) fn new(
        owner: FunctionRef,
        owner_module: ModuleRef,
        function_source: BodySource,
        line_index: &'a LineIndex,
        interner: &'a mut NameInterner,
    ) -> Self {
        Self {
            owner,
            owner_module,
            function_source,
            line_index,
            interner,
            builder: BodyBuilder::default(),
        }
    }

    pub(super) fn lower(mut self, function: ast::Fn, body: ast::BlockExpr) -> BodyData {
        // Parameters live in the function's outer lexical scope. The body block gets a child scope
        // so locals do not appear before the function boundary.
        let param_scope = self.builder.alloc_scope(None);
        let params = self.lower_params(function.param_list(), param_scope);
        let root_expr = self.lower_block_expr(body, param_scope);

        BodyData::new(
            self.owner,
            self.owner_module,
            self.function_source,
            param_scope,
            root_expr,
            params,
            self.builder,
        )
    }
}

impl FunctionBodyLowering<'_> {
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
            resolution: BodyResolution::Unknown,
            ty: IndexedTy::Unknown,
        })
    }

    pub(super) fn source(&self, syntax: &rg_syntax::SyntaxNode) -> BodySource {
        source_for(self.function_source.file_id, syntax)
    }
}
