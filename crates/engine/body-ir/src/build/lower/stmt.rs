//! Lowering for function parameters, blocks, statements, and body-local items.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _},
};

use rg_item_tree::{
    ConstItem, Documentation, EnumItem, FunctionItem, ImplItem, StaticItem, StructItem, TraitItem,
    TypeAliasItem, TypeRef, UnionItem,
};

use crate::ir::{
    BindingData, BindingId, BindingKind, BodyFunctionData, BodyFunctionId, BodyFunctionOwner,
    BodyImplData, BodyImplId, BodyItemData, BodyItemDeclaration, BodyItemId, BodyItemKind,
    BodyItemOwner, BodyTy, BodyValueItemData, BodyValueItemDeclaration, BodyValueItemId,
    BodyValueItemKind, BodyValueItemOwner, ExprBlockKind, ExprId, ExprKind, ScopeId, StmtData,
    StmtId, StmtKind,
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
        let kind = self.lower_block_kind(&block);
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

        let label = self.lower_label(block.label());
        self.alloc_expr(
            block.syntax(),
            block_scope,
            ExprKind::Block {
                kind,
                label,
                scope: block_scope,
                statements,
                tail,
            },
        )
    }

    // Read block modifier tokens directly so `move` stays attached to async/gen blocks.
    fn lower_block_kind(&mut self, block: &ast::BlockExpr) -> ExprBlockKind {
        let move_capture = block.move_token().is_some();

        if block.gen_token().is_some() {
            return if block.async_token().is_some() {
                ExprBlockKind::AsyncGen { move_capture }
            } else {
                ExprBlockKind::Gen { move_capture }
            };
        }

        if block.async_token().is_some() {
            return ExprBlockKind::Async { move_capture };
        }

        if block.unsafe_token().is_some() {
            return ExprBlockKind::Unsafe;
        }

        if let Some(modifier) = block.try_block_modifier() {
            if modifier.try_token().is_some() {
                return ExprBlockKind::Try {
                    bikeshed: modifier.bikeshed_token().is_some(),
                    result_ty: modifier
                        .ty()
                        .map(|ty| TypeRef::from_ast(ty, self.line_index, self.interner)),
                };
            }
        }

        if block.const_token().is_some() {
            return ExprBlockKind::Const;
        }

        ExprBlockKind::Plain
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
            ast::Item::Enum(item) => self
                .lower_local_enum_item(item, scope)
                .map(|item| StmtKind::Item { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Union(item) => self
                .lower_local_union_item(item, scope)
                .map(|item| StmtKind::Item { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::TypeAlias(item) => self
                .lower_local_type_alias_item(item, scope)
                .map(|item| StmtKind::Item { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Trait(item) => self
                .lower_local_trait_item(item, scope)
                .map(|item| StmtKind::Item { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Const(item) => self
                .lower_local_const_item(item, BodyValueItemOwner::LocalScope(scope), scope)
                .map(|item| StmtKind::ValueItem { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Static(item) => self
                .lower_local_static_item(item, BodyValueItemOwner::LocalScope(scope), scope)
                .map(|item| StmtKind::ValueItem { item })
                .unwrap_or(StmtKind::ItemIgnored),
            ast::Item::Fn(item) => self
                .lower_local_function_item(item, BodyFunctionOwner::LocalScope(scope))
                .map(|function| StmtKind::Function { function })
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

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner: BodyItemOwner::LocalScope(scope),
            kind: BodyItemKind::Struct,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyItemDeclaration::Struct(StructItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_enum_item(&mut self, item: ast::Enum, scope: ScopeId) -> Option<BodyItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner: BodyItemOwner::LocalScope(scope),
            kind: BodyItemKind::Enum,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyItemDeclaration::Enum(EnumItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_union_item(&mut self, item: ast::Union, scope: ScopeId) -> Option<BodyItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner: BodyItemOwner::LocalScope(scope),
            kind: BodyItemKind::Union,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyItemDeclaration::Union(UnionItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_type_alias_item(
        &mut self,
        item: ast::TypeAlias,
        scope: ScopeId,
    ) -> Option<BodyItemId> {
        self.lower_type_alias_item(item, scope, BodyItemOwner::LocalScope(scope))
    }

    fn lower_type_alias_item(
        &mut self,
        item: ast::TypeAlias,
        scope: ScopeId,
        owner: BodyItemOwner,
    ) -> Option<BodyItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner,
            kind: BodyItemKind::TypeAlias,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyItemDeclaration::TypeAlias(TypeAliasItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_trait_item(&mut self, item: ast::Trait, scope: ScopeId) -> Option<BodyItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_item(BodyItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner: BodyItemOwner::LocalScope(scope),
            kind: BodyItemKind::Trait,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyItemDeclaration::Trait(TraitItem::from_ast(
                &item,
                Vec::new(),
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_const_item(
        &mut self,
        item: ast::Const,
        owner: BodyValueItemOwner,
        scope: ScopeId,
    ) -> Option<BodyValueItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_value_item(BodyValueItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner,
            kind: BodyValueItemKind::Const,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyValueItemDeclaration::Const(ConstItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
        }))
    }

    fn lower_local_static_item(
        &mut self,
        item: ast::Static,
        owner: BodyValueItemOwner,
        scope: ScopeId,
    ) -> Option<BodyValueItemId> {
        let name = item.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_value_item(BodyValueItemData {
            source: self.source(item.syntax()),
            name_source,
            scope,
            owner,
            kind: BodyValueItemKind::Static,
            name,
            docs: Documentation::from_ast(&item),
            declaration: BodyValueItemDeclaration::Static(StaticItem::from_ast(
                &item,
                self.line_index,
                self.interner,
            )),
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
            consts: Vec::new(),
            types: Vec::new(),
        });

        let mut functions = Vec::new();
        let mut consts = Vec::new();
        let mut types = Vec::new();
        for item in item
            .assoc_item_list()
            .into_iter()
            .flat_map(|item_list| item_list.assoc_items())
        {
            match item {
                ast::AssocItem::Fn(function) => {
                    if let Some(function) = self
                        .lower_local_function_item(function, BodyFunctionOwner::LocalImpl(impl_id))
                    {
                        functions.push(function);
                    }
                }
                ast::AssocItem::Const(item) => {
                    if let Some(item) = self.lower_local_const_item(
                        item,
                        BodyValueItemOwner::LocalImpl(impl_id),
                        scope,
                    ) {
                        consts.push(item);
                    }
                }
                ast::AssocItem::TypeAlias(item) => {
                    if let Some(item) =
                        self.lower_type_alias_item(item, scope, BodyItemOwner::LocalImpl(impl_id))
                    {
                        types.push(item);
                    }
                }
                ast::AssocItem::MacroCall(_) => {}
            }
        }
        self.builder
            .set_local_impl_items(impl_id, functions, consts, types);

        Some(impl_id)
    }

    fn lower_local_function_item(
        &mut self,
        function: ast::Fn,
        owner: BodyFunctionOwner,
    ) -> Option<BodyFunctionId> {
        let name = function.name()?;
        let name_source = self.source(name.syntax());
        let name = self.intern_ast_name(name);

        Some(self.builder.alloc_local_function(BodyFunctionData {
            source: self.source(function.syntax()),
            name_source,
            owner,
            name,
            docs: Documentation::from_ast(&function),
            declaration: FunctionItem::from_ast(&function, self.line_index, self.interner),
        }))
    }

    fn lower_let_statement(&mut self, statement: ast::LetStmt, scope: ScopeId) -> StmtId {
        // Initializers and `let else` blocks cannot see bindings introduced by the successful
        // pattern, so lower them before allocating those bindings.
        let initializer = statement
            .initializer()
            .map(|initializer| self.lower_expr(initializer, scope));
        let else_branch = statement
            .let_else()
            .and_then(|else_branch| else_branch.block_expr())
            .map(|block| self.lower_block_expr(block, scope));
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
                else_branch,
            },
        })
    }
}
