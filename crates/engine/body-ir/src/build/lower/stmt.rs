//! Lowering for function parameters, blocks, statements, and body-local items.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasModuleItem as _, HasName as _, HasVisibility as _},
};

use rg_ir_model::{ExprId, FunctionParamData, ScopeId, StmtId, items::Mutability};
use rg_item_tree::{
    ConstItem, Documentation, EnumItem, ExternCrateItem, FromAst as _, FunctionItem, ImplItem,
    ImplItemContext, InnerDocs, ItemKind, ItemNode, ItemTreeId, MacroUseAttr, MaybeFromAst,
    ModuleItem, ModuleSource, OuterDocs, StaticItem, StructItem, TraitItem, TraitItemContext,
    TypeAliasItem, TypeRef, UnionItem, UseItem, VisibilityLevel,
};
use rg_parse::Span;
use rg_text::Name;

use crate::ir::{
    BindingData, BindingKind, BodySelfParamKind, ExprBlockKind, ExprKind, StmtData, StmtKind,
};

use super::body::BodyLowering;

impl BodyLowering<'_> {
    pub(super) fn lower_params(
        &mut self,
        param_list: Option<ast::ParamList>,
        param_scope: ScopeId,
    ) -> Vec<FunctionParamData> {
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
                .map(|param| self.lower_param(param, param_scope)),
        );

        params
    }

    fn lower_self_param(&mut self, param: ast::SelfParam, scope: ScopeId) -> FunctionParamData {
        let source = self.source(param.syntax());
        let annotation = param
            .ty()
            .map(|ty| TypeRef::from_ast(&ty, (self.line_index, &mut *self.interner)));
        let self_kind = if annotation.is_some() {
            BodySelfParamKind::Explicit
        } else if param.amp_token().is_some() {
            BodySelfParamKind::Reference {
                mutability: Mutability::from_mut_token(param.mut_token().is_some()),
            }
        } else {
            BodySelfParamKind::Value
        };
        let binding = self.builder.alloc_binding(BindingData {
            source,
            name_span: param
                .name()
                .map(|name| self.source(name.syntax()).span)
                .or(Some(source.span)),
            scope,
            kind: BindingKind::SelfParam(self_kind),
            name: Some(self.interner.intern("self")),
            annotation: annotation.clone(),
        });

        FunctionParamData {
            source,
            pat: None,
            bindings: vec![binding],
            annotation,
        }
    }

    fn lower_param(&mut self, param: ast::Param, scope: ScopeId) -> FunctionParamData {
        let source = self.source(param.syntax());
        let annotation = param
            .ty()
            .map(|ty| TypeRef::from_ast(&ty, (self.line_index, &mut *self.interner)));
        let (pat, bindings) = match param.pat() {
            Some(pat) => self.lower_pat(pat, scope, BindingKind::Param, annotation.clone()),
            None => (
                None,
                vec![self.builder.alloc_binding(BindingData {
                    source,
                    name_span: None,
                    scope,
                    kind: BindingKind::Param,
                    name: None,
                    annotation: annotation.clone(),
                })],
            ),
        };

        FunctionParamData {
            source,
            pat,
            bindings,
            annotation,
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

        if let Some(modifier) = block.try_block_modifier()
            && modifier.try_token().is_some()
        {
            return ExprBlockKind::Try {
                bikeshed: modifier.bikeshed_token().is_some(),
                result_ty: modifier
                    .ty()
                    .map(|ty| TypeRef::from_ast(&ty, (self.line_index, &mut *self.interner))),
            };
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

        let kind = self
            .lower_source_item(&item)
            .map(|node| self.builder.alloc_scope_source_item(scope, node))
            .map(|item| StmtKind::Item { item })
            .unwrap_or(StmtKind::ItemIgnored);
        self.builder.alloc_statement(StmtData { source, kind })
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
            .map(|ty| TypeRef::from_ast(&ty, (self.line_index, &mut *self.interner)));
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

    fn lower_source_item(&mut self, item: &ast::Item) -> Option<ItemNode> {
        match item {
            ast::Item::AsmExpr(item) => Some(self.named_source_item_node(
                ItemKind::AsmExpr,
                None,
                VisibilityLevel::Private,
                None,
                item.syntax(),
            )),
            ast::Item::Const(item) => {
                let kind = ItemKind::Const(ConstItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Enum(item) => {
                let kind = ItemKind::Enum(EnumItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::ExternBlock(item) => Some(self.named_source_item_node(
                ItemKind::ExternBlock,
                None,
                VisibilityLevel::Private,
                None,
                item.syntax(),
            )),
            ast::Item::ExternCrate(item) => {
                let (name, name_span) = self.source_name_ref(item.name_ref());
                let kind =
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(item, &mut *self.interner));
                Some(Self::source_item_node(
                    kind,
                    name,
                    name_span,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    self.source(item.syntax()),
                ))
            }
            ast::Item::Fn(item) => {
                let kind = ItemKind::Function(FunctionItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Impl(item) => {
                let items = self.lower_source_assoc_items(item.assoc_item_list());
                let kind = ItemKind::Impl(ImplItem::from_ast(
                    item,
                    ImplItemContext {
                        items,
                        line_index: self.line_index,
                        interner: &mut *self.interner,
                    },
                ));
                Some(self.named_source_item_node(
                    kind,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Module(item) => {
                let module_item = self.lower_source_module_item(item);
                Some(self.named_source_item_node(
                    ItemKind::Module(module_item),
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Static(item) => {
                let kind = ItemKind::Static(StaticItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Struct(item) => {
                let kind = ItemKind::Struct(StructItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Trait(item) => {
                let items = self.lower_source_assoc_items(item.assoc_item_list());
                let kind = ItemKind::Trait(TraitItem::from_ast(
                    item,
                    TraitItemContext {
                        items,
                        line_index: self.line_index,
                        interner: &mut *self.interner,
                    },
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::TypeAlias(item) => {
                let kind = ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Union(item) => {
                let kind = ItemKind::Union(UnionItem::from_ast(
                    item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::Use(item) => {
                let kind = ItemKind::Use(UseItem::from_ast(item, &mut *self.interner));
                Some(self.named_source_item_node(
                    kind,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::Item::MacroCall(_) | ast::Item::MacroDef(_) | ast::Item::MacroRules(_) => {
                // TODO: Lower body-local macro items after body source collection has access to
                // the macro expansion context used by file item-tree lowering.
                None
            }
        }
    }

    fn lower_source_module_item(&mut self, item: &ast::Module) -> ModuleItem {
        let source = match item.item_list() {
            Some(item_list) => ModuleSource::Inline {
                items: self.lower_source_child_items(item_list.items()),
            },
            None => ModuleSource::OutOfLine {
                // TODO: Resolve out-of-line body-local modules once this lowering pass knows the
                // module file context that normal item-tree lowering receives.
                definition_file: None,
            },
        };

        ModuleItem {
            inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(item, InnerDocs),
            macro_use: MacroUseAttr::maybe_from_ast(item, &mut *self.interner),
            source,
        }
    }

    fn lower_source_child_items(
        &mut self,
        items: impl IntoIterator<Item = ast::Item>,
    ) -> Vec<ItemTreeId> {
        let mut item_ids = Vec::new();
        for item in items {
            if let Some(node) = self.lower_source_item(&item) {
                item_ids.push(self.builder.alloc_scopeless_source_item(node));
            }
        }
        item_ids
    }

    fn lower_source_assoc_items(
        &mut self,
        item_list: Option<ast::AssocItemList>,
    ) -> Vec<ItemTreeId> {
        let Some(item_list) = item_list else {
            return Vec::new();
        };

        let mut item_ids = Vec::new();
        for item in item_list.assoc_items() {
            if let Some(node) = self.lower_source_assoc_item(item) {
                item_ids.push(self.builder.alloc_scopeless_source_item(node));
            }
        }
        item_ids
    }

    fn lower_source_assoc_item(&mut self, item: ast::AssocItem) -> Option<ItemNode> {
        match item {
            ast::AssocItem::Const(item) => {
                let kind = ItemKind::Const(ConstItem::from_ast(
                    &item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::AssocItem::Fn(item) => {
                let kind = ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::AssocItem::TypeAlias(item) => {
                let kind = ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    (self.line_index, &mut *self.interner),
                ));
                Some(self.named_source_item_node(
                    kind,
                    item.name(),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&item, OuterDocs),
                    item.syntax(),
                ))
            }
            ast::AssocItem::MacroCall(_) => None,
        }
    }

    fn named_source_item_node(
        &mut self,
        kind: ItemKind,
        name: Option<ast::Name>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        syntax: &rg_syntax::SyntaxNode,
    ) -> ItemNode {
        let (name, name_span) = self.source_name(name);
        Self::source_item_node(kind, name, name_span, visibility, docs, self.source(syntax))
    }

    fn source_item_node(
        kind: ItemKind,
        name: Option<Name>,
        name_span: Option<Span>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        source: crate::ir::BodySource,
    ) -> ItemNode {
        ItemNode::source(
            kind,
            name,
            name_span,
            visibility,
            docs,
            source.span,
            source.file_id,
        )
    }

    fn source_name(&mut self, name: Option<ast::Name>) -> (Option<Name>, Option<Span>) {
        let Some(name) = name else {
            return (None, None);
        };

        let span = self.source(name.syntax()).span;
        (Some(self.intern_ast_name(name)), Some(span))
    }

    fn source_name_ref(&mut self, name_ref: Option<ast::NameRef>) -> (Option<Name>, Option<Span>) {
        let Some(name_ref) = name_ref else {
            return (None, None);
        };

        let span = self.source(name_ref.syntax()).span;
        (Some(self.intern_ast_name_ref(name_ref)), Some(span))
    }
}
