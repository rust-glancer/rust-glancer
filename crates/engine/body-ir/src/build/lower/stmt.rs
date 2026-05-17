//! Lowering for function parameters, blocks, statements, and body-local items.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _},
};

use rg_item_tree::{Documentation, FieldList, FunctionItem, GenericParams, ImplItem, TypeRef};

use crate::ir::{
    BindingData, BindingId, BindingKind, BodyFunctionData, BodyFunctionId, BodyFunctionOwner,
    BodyImplData, BodyImplId, BodyItemData, BodyItemId, BodyItemKind, BodyTy, ExprId, ExprKind,
    ScopeId, StmtData, StmtId, StmtKind,
};

use super::function::FunctionBodyLowering;

impl FunctionBodyLowering<'_> {
    pub(super) fn lower_params(
        &mut self,
        param_list: Option<ast::ParamList>,
        param_scope: ScopeId,
    ) -> Vec<BindingId> {
        let Some(param_list) = param_list else {
            return Vec::new();
        };

        let mut params = Vec::new();
        if let Some(self_param) = param_list.self_param() {
            params.push(self.lower_self_param(self_param, param_scope));
        }

        params.extend(
            param_list
                .params()
                .flat_map(|param| self.lower_param(param, param_scope)),
        );

        params
    }

    fn lower_self_param(&mut self, param: ast::SelfParam, scope: ScopeId) -> BindingId {
        let source = self.source(param.syntax());
        let annotation = param
            .ty()
            .map(|ty| TypeRef::from_ast(ty, self.line_index, self.interner));
        self.builder.alloc_binding(BindingData {
            source,
            scope,
            kind: BindingKind::SelfParam,
            name: Some(self.interner.intern("self")),
            annotation,
            ty: BodyTy::Unknown,
        })
    }

    fn lower_param(&mut self, param: ast::Param, scope: ScopeId) -> Vec<BindingId> {
        let annotation = param
            .ty()
            .map(|ty| TypeRef::from_ast(ty, self.line_index, self.interner));
        match param.pat() {
            Some(pat) => self.lower_pat(pat, scope, BindingKind::Param, annotation).1,
            None => vec![self.builder.alloc_binding(BindingData {
                source: self.source(param.syntax()),
                scope,
                kind: BindingKind::Param,
                name: None,
                annotation,
                ty: BodyTy::Unknown,
            })],
        }
    }

    pub(super) fn lower_block_expr(
        &mut self,
        block: ast::BlockExpr,
        parent_scope: ScopeId,
    ) -> ExprId {
        let block_scope = self.builder.alloc_scope(Some(parent_scope));
        let mut statements = Vec::new();
        let mut tail = None;

        if let Some(stmt_list) = block.stmt_list() {
            statements.extend(
                stmt_list
                    .statements()
                    .map(|statement| self.lower_statement(statement, block_scope)),
            );
            tail = stmt_list
                .tail_expr()
                .map(|tail_expr| self.lower_expr(tail_expr, block_scope));
        }

        self.alloc_expr(
            block.syntax(),
            block_scope,
            ExprKind::Block {
                scope: block_scope,
                statements,
                tail,
            },
        )
    }

    fn lower_statement(&mut self, statement: ast::Stmt, scope: ScopeId) -> StmtId {
        match statement {
            ast::Stmt::LetStmt(statement) => self.lower_let_statement(statement, scope),
            ast::Stmt::ExprStmt(statement) => {
                let expr = statement.expr().map(|expr| self.lower_expr(expr, scope));
                self.builder.alloc_statement(StmtData {
                    source: self.source(statement.syntax()),
                    kind: match expr {
                        Some(expr) => StmtKind::Expr {
                            expr,
                            has_semicolon: statement.semicolon_token().is_some(),
                        },
                        None => StmtKind::ItemIgnored,
                    },
                })
            }
            ast::Stmt::Item(item) => self.lower_item_statement(item, scope),
        }
    }

    fn lower_item_statement(&mut self, item: ast::Item, scope: ScopeId) -> StmtId {
        let source = self.source(item.syntax());
        // Body IR only keeps local items that can affect current editor queries. Other item
        // statements remain represented as ignored statements so source layout stays stable.
        let kind = match item {
            ast::Item::Struct(item) => self
                .lower_local_struct_item(item, scope)
                .map(|item| StmtKind::Item { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Impl(item) => self
                .lower_local_impl_item(item, scope)
                .map(|impl_id| StmtKind::Impl { impl_id })
                .unwrap_or(StmtKind::ItemIgnored),
            _ => StmtKind::ItemIgnored,
        };

        self.builder.alloc_statement(StmtData { source, kind })
    }

    fn lower_local_struct_item(&mut self, item: ast::Struct, scope: ScopeId) -> Option<BodyItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);
        let fields = FieldList::from_ast(item.field_list(), self.line_index, self.interner);

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            kind: BodyItemKind::Struct,
            name,
            docs: Documentation::from_ast(&item),
            generics: GenericParams::from_ast(&item, self.line_index, self.interner),
            fields,
        }))
    }

    fn lower_local_impl_item(&mut self, item: ast::Impl, scope: ScopeId) -> Option<BodyImplId> {
        let impl_item = ImplItem::from_ast(&item, Vec::new(), self.line_index, self.interner);
        let impl_id = self.builder.alloc_local_impl(BodyImplData {
            source: self.source(item.syntax()),
            scope,
            generics: impl_item.generics,
            trait_ref: impl_item.trait_ref,
            self_ty: impl_item.self_ty,
            self_item: None,
            functions: Vec::new(),
        });

        let functions = item
            .assoc_item_list()
            .into_iter()
            .flat_map(|item_list| item_list.assoc_items())
            .filter_map(|item| self.lower_local_assoc_function(item, impl_id))
            .collect::<Vec<_>>();
        self.builder.set_local_impl_functions(impl_id, functions);

        Some(impl_id)
    }

    fn lower_local_assoc_function(
        &mut self,
        item: ast::AssocItem,
        impl_id: BodyImplId,
    ) -> Option<BodyFunctionId> {
        let ast::AssocItem::Fn(function) = item else {
            return None;
        };
        let name = function.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_function(BodyFunctionData {
            source: self.source(function.syntax()),
            name_source,
            owner: BodyFunctionOwner::LocalImpl(impl_id),
            name,
            docs: Documentation::from_ast(&function),
            declaration: FunctionItem::from_ast(&function, self.line_index, self.interner),
        }))
    }

    fn lower_let_statement(&mut self, statement: ast::LetStmt, scope: ScopeId) -> StmtId {
        // Initializers cannot see the binding introduced by their own `let`, so lower the
        // initializer before allocating the binding.
        let initializer = statement
            .initializer()
            .map(|initializer| self.lower_expr(initializer, scope));
        let annotation = statement
            .ty()
            .map(|ty| TypeRef::from_ast(ty, self.line_index, self.interner));
        let bindings = statement
            .pat()
            .map(|pat| self.lower_pat(pat, scope, BindingKind::Let, annotation.clone()))
            .unwrap_or_default();
        let (pat, bindings) = bindings;

        self.builder.alloc_statement(StmtData {
            source: self.source(statement.syntax()),
            kind: StmtKind::Let {
                scope,
                pat,
                bindings,
                annotation,
                initializer,
            },
        })
    }
}
