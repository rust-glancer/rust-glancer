//! Expression lowering for syntax that Body IR models directly.

use rg_syntax::{
    AstNode as _,
    ast::{self, BinaryOp, ElseBranch, HasArgList as _, HasLoopBody as _, LogicOp},
};

use rg_item_tree::FieldKey;
use rg_parse::Span;

use crate::ir::{
    BindingKind, ExprId, ExprKind, ExprWrapperKind, LiteralKind, MatchArmData, RecordExprField,
    ScopeId,
};

use super::function::FunctionBodyLowering;

impl FunctionBodyLowering<'_> {
    pub(super) fn lower_expr(&mut self, expr: ast::Expr, scope: ScopeId) -> ExprId {
        match expr {
            ast::Expr::BlockExpr(block) => self.lower_block_expr(block, scope),
            ast::Expr::CallExpr(call) => self.lower_call_expr(call, scope),
            ast::Expr::FieldExpr(field) => self.lower_field_expr(field, scope),
            ast::Expr::ForExpr(for_expr) => self.lower_for_expr(for_expr, scope),
            ast::Expr::IfExpr(if_expr) => self.lower_if_expr(if_expr, scope),
            ast::Expr::LetExpr(let_expr) => {
                let let_scope = self.builder.alloc_scope(Some(scope));
                self.lower_let_expr(let_expr, let_scope)
            }
            ast::Expr::Literal(literal) => self.lower_literal(literal, scope),
            ast::Expr::LoopExpr(loop_expr) => self.lower_loop_expr(loop_expr, scope),
            ast::Expr::MatchExpr(match_expr) => self.lower_match_expr(match_expr, scope),
            ast::Expr::MethodCallExpr(method_call) => {
                self.lower_method_call_expr(method_call, scope)
            }
            ast::Expr::BreakExpr(break_expr) => self.lower_break_expr(break_expr, scope),
            ast::Expr::ContinueExpr(continue_expr) => {
                self.lower_continue_expr(continue_expr, scope)
            }
            ast::Expr::RecordExpr(record) => self.lower_record_expr(record, scope),
            ast::Expr::AwaitExpr(await_expr) => self.lower_wrapper_expr(
                await_expr.syntax(),
                await_expr.expr(),
                scope,
                ExprWrapperKind::Await,
            ),
            ast::Expr::ParenExpr(paren) => match paren.expr() {
                Some(inner) => self.lower_wrapper_expr(
                    paren.syntax(),
                    Some(inner),
                    scope,
                    ExprWrapperKind::Paren,
                ),
                None => {
                    self.lower_wrapper_expr(paren.syntax(), None, scope, ExprWrapperKind::Paren)
                }
            },
            ast::Expr::PathExpr(path) => self.lower_path_expr(path, scope),
            ast::Expr::PrefixExpr(prefix) => match prefix.expr() {
                Some(inner) => self.lower_passthrough_unknown(prefix.syntax(), vec![inner], scope),
                None => self.lower_unknown_expr(prefix.syntax(), scope),
            },
            ast::Expr::RefExpr(ref_expr) => match ref_expr.expr() {
                Some(inner) => self.lower_wrapper_expr(
                    ref_expr.syntax(),
                    Some(inner),
                    scope,
                    ExprWrapperKind::Ref,
                ),
                None => {
                    self.lower_wrapper_expr(ref_expr.syntax(), None, scope, ExprWrapperKind::Ref)
                }
            },
            ast::Expr::ReturnExpr(return_expr) => match return_expr.expr() {
                Some(inner) => self.lower_wrapper_expr(
                    return_expr.syntax(),
                    Some(inner),
                    scope,
                    ExprWrapperKind::Return,
                ),
                None => self.lower_wrapper_expr(
                    return_expr.syntax(),
                    None,
                    scope,
                    ExprWrapperKind::Return,
                ),
            },
            ast::Expr::TryExpr(try_expr) => self.lower_wrapper_expr(
                try_expr.syntax(),
                try_expr.expr(),
                scope,
                ExprWrapperKind::Try,
            ),
            ast::Expr::WhileExpr(while_expr) => self.lower_while_expr(while_expr, scope),
            // Unsupported expressions still lower their direct expression children so cursor and
            // type queries can work inside syntax the IR does not model yet.
            expr => self.lower_unknown_with_direct_children(expr.syntax(), scope),
        }
    }

    fn lower_call_expr(&mut self, call: ast::CallExpr, scope: ScopeId) -> ExprId {
        let callee = call.expr().map(|callee| self.lower_expr(callee, scope));
        let args = call
            .arg_list()
            .into_iter()
            .flat_map(|args| args.args())
            .map(|arg| self.lower_expr(arg, scope))
            .collect();

        self.alloc_expr(call.syntax(), scope, ExprKind::Call { callee, args })
    }

    fn lower_if_expr(&mut self, if_expr: ast::IfExpr, scope: ScopeId) -> ExprId {
        let (condition, then_parent_scope) = self.lower_condition_expr(if_expr.condition(), scope);
        let then_branch = if_expr
            .then_branch()
            .map(|then_branch| self.lower_block_expr(then_branch, then_parent_scope));
        let else_branch = if_expr.else_branch().map(|else_branch| match else_branch {
            ElseBranch::Block(block) => self.lower_block_expr(block, scope),
            ElseBranch::IfExpr(if_expr) => self.lower_if_expr(if_expr, scope),
        });

        self.alloc_expr(
            if_expr.syntax(),
            scope,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            },
        )
    }

    fn lower_condition_expr(
        &mut self,
        condition: Option<ast::Expr>,
        scope: ScopeId,
    ) -> (Option<ExprId>, ScopeId) {
        match condition {
            Some(ast::Expr::LetExpr(let_expr)) => {
                let condition_scope = self.builder.alloc_scope(Some(scope));
                (
                    Some(self.lower_let_expr(let_expr, condition_scope)),
                    condition_scope,
                )
            }
            Some(ast::Expr::BinExpr(bin_expr))
                if matches!(bin_expr.op_kind(), Some(BinaryOp::LogicOp(LogicOp::And))) =>
            {
                let (lhs, rhs_scope) = self.lower_condition_expr(bin_expr.lhs(), scope);
                let (rhs, success_scope) = self.lower_condition_expr(bin_expr.rhs(), rhs_scope);
                let children = [lhs, rhs].into_iter().flatten().collect();

                // Until we have real binary IR, preserve the source node as an unsupported
                // expression while still lowering each side in its Rust lexical scope.
                (
                    Some(self.alloc_expr(bin_expr.syntax(), scope, ExprKind::Unknown { children })),
                    success_scope,
                )
            }
            Some(condition) => (Some(self.lower_expr(condition, scope)), scope),
            None => (None, scope),
        }
    }

    fn lower_let_expr(&mut self, let_expr: ast::LetExpr, scope: ScopeId) -> ExprId {
        // The scrutinee is evaluated before the pattern bindings exist, which keeps
        // `if let x = x` pointed at the outer `x` on the right-hand side.
        let initializer = let_expr.expr().map(|expr| self.lower_expr(expr, scope));
        let (pat, bindings) = let_expr
            .pat()
            .map(|pat| self.lower_pat(pat, scope, BindingKind::Let, None))
            .unwrap_or_default();

        self.alloc_expr(
            let_expr.syntax(),
            scope,
            ExprKind::Let {
                scope,
                pat,
                bindings,
                initializer,
            },
        )
    }

    fn lower_loop_expr(&mut self, loop_expr: ast::LoopExpr, scope: ScopeId) -> ExprId {
        let label = self.lower_label(loop_expr.label());
        let body = loop_expr
            .loop_body()
            .map(|body| self.lower_block_expr(body, scope));

        self.alloc_expr(loop_expr.syntax(), scope, ExprKind::Loop { label, body })
    }

    fn lower_while_expr(&mut self, while_expr: ast::WhileExpr, scope: ScopeId) -> ExprId {
        let label = self.lower_label(while_expr.label());
        let (condition, body_parent_scope) =
            self.lower_condition_expr(while_expr.condition(), scope);
        let body = while_expr
            .loop_body()
            .map(|body| self.lower_block_expr(body, body_parent_scope));

        self.alloc_expr(
            while_expr.syntax(),
            scope,
            ExprKind::While {
                label,
                condition,
                body,
            },
        )
    }

    fn lower_for_expr(&mut self, for_expr: ast::ForExpr, scope: ScopeId) -> ExprId {
        let label = self.lower_label(for_expr.label());
        let iterable = for_expr
            .iterable()
            .map(|iterable| self.lower_expr(iterable, scope));
        let loop_scope = self.builder.alloc_scope(Some(scope));
        let (pat, bindings) = for_expr
            .pat()
            .map(|pat| self.lower_pat(pat, loop_scope, BindingKind::Let, None))
            .unwrap_or_default();
        let body = for_expr
            .loop_body()
            .map(|body| self.lower_block_expr(body, loop_scope));

        self.alloc_expr(
            for_expr.syntax(),
            scope,
            ExprKind::For {
                label,
                scope: loop_scope,
                pat,
                bindings,
                iterable,
                body,
            },
        )
    }

    fn lower_break_expr(&mut self, break_expr: ast::BreakExpr, scope: ScopeId) -> ExprId {
        let label = self.lower_lifetime_label(break_expr.lifetime());
        let value = break_expr.expr().map(|expr| self.lower_expr(expr, scope));

        self.alloc_expr(break_expr.syntax(), scope, ExprKind::Break { label, value })
    }

    fn lower_continue_expr(&mut self, continue_expr: ast::ContinueExpr, scope: ScopeId) -> ExprId {
        let label = self.lower_lifetime_label(continue_expr.lifetime());

        self.alloc_expr(continue_expr.syntax(), scope, ExprKind::Continue { label })
    }

    fn lower_match_expr(&mut self, match_expr: ast::MatchExpr, scope: ScopeId) -> ExprId {
        let scrutinee = match_expr
            .expr()
            .map(|scrutinee| self.lower_expr(scrutinee, scope));
        let arms = match_expr
            .match_arm_list()
            .into_iter()
            .flat_map(|arm_list| arm_list.arms())
            .map(|arm| self.lower_match_arm(arm, scope))
            .collect();

        self.alloc_expr(
            match_expr.syntax(),
            scope,
            ExprKind::Match { scrutinee, arms },
        )
    }

    fn lower_match_arm(&mut self, arm: ast::MatchArm, parent_scope: ScopeId) -> MatchArmData {
        let scope = self.builder.alloc_scope(Some(parent_scope));
        let pat = arm
            .pat()
            .map(|pat| self.lower_pat(pat, scope, BindingKind::Let, None).0)
            .unwrap_or_default();
        let expr = arm.expr().map(|expr| self.lower_expr(expr, scope));

        MatchArmData { pat, scope, expr }
    }

    fn lower_method_call_expr(
        &mut self,
        method_call: ast::MethodCallExpr,
        scope: ScopeId,
    ) -> ExprId {
        let receiver = method_call
            .receiver()
            .map(|receiver| self.lower_expr(receiver, scope));
        let dot_span = method_call
            .dot_token()
            .map(|dot| Span::from_text_range(dot.text_range()));
        let name_ref = method_call.name_ref();
        let method_name = name_ref
            .clone()
            .map(|name| self.intern_ast_name_ref(name))
            .unwrap_or_else(|| self.interner.intern("<missing>"));
        let method_name_span = name_ref
            .as_ref()
            .map(|name| self.source(name.syntax()).span);
        let args = method_call
            .arg_list()
            .into_iter()
            .flat_map(|args| args.args())
            .map(|arg| self.lower_expr(arg, scope))
            .collect();

        self.alloc_expr(
            method_call.syntax(),
            scope,
            ExprKind::MethodCall {
                receiver,
                dot_span,
                method_name,
                method_name_span,
                args,
            },
        )
    }

    fn lower_field_expr(&mut self, field: ast::FieldExpr, scope: ScopeId) -> ExprId {
        let base = field.expr().map(|base| self.lower_expr(base, scope));
        let dot_span = field
            .dot_token()
            .map(|dot| Span::from_text_range(dot.text_range()));
        let (field_key, field_span) = if let Some(index) = field.index_token() {
            (
                index.text().parse::<usize>().ok().map(FieldKey::Tuple),
                Some(Span::from_text_range(index.text_range())),
            )
        } else if let Some(name) = field.name_ref() {
            let field_key = name
                .as_tuple_field()
                .map(FieldKey::Tuple)
                .unwrap_or_else(|| FieldKey::Named(self.intern_ast_name_ref(name.clone())));
            (Some(field_key), Some(self.source(name.syntax()).span))
        } else {
            (None, None)
        };

        self.alloc_expr(
            field.syntax(),
            scope,
            ExprKind::Field {
                base,
                dot_span,
                field: field_key,
                field_span,
            },
        )
    }

    fn lower_record_expr(&mut self, record: ast::RecordExpr, scope: ScopeId) -> ExprId {
        let mut fields = Vec::new();
        let mut spread = None;
        let field_list_span = record
            .record_expr_field_list()
            .as_ref()
            .map(|field_list| self.source(field_list.syntax()).span);

        if let Some(field_list) = record.record_expr_field_list() {
            fields.extend(
                field_list
                    .fields()
                    .filter_map(|field| self.lower_record_expr_field(field, scope)),
            );
            spread = field_list.spread().map(|expr| self.lower_expr(expr, scope));
        }
        let path = record.path().and_then(|path| self.lower_body_path(path));

        self.alloc_expr(
            record.syntax(),
            scope,
            ExprKind::Record {
                path,
                field_list_span,
                fields,
                spread,
            },
        )
    }

    fn lower_record_expr_field(
        &mut self,
        field: ast::RecordExprField,
        scope: ScopeId,
    ) -> Option<RecordExprField> {
        let field_name = field.field_name()?;
        let key_span = self.source(field_name.syntax()).span;
        let key = FieldKey::Named(self.intern_ast_name_ref(field_name));
        let source_span = self.source(field.syntax()).span;
        let value = field.expr().map(|expr| self.lower_expr(expr, scope));

        Some(RecordExprField {
            key,
            key_span,
            source_span,
            value,
        })
    }

    fn lower_literal(&mut self, literal: ast::Literal, scope: ScopeId) -> ExprId {
        let kind = LiteralKind::from_ast(&literal);

        self.alloc_expr(literal.syntax(), scope, ExprKind::Literal { kind })
    }

    fn lower_path_expr(&mut self, expr: ast::PathExpr, scope: ScopeId) -> ExprId {
        let Some(path) = expr.path().and_then(|path| self.lower_body_path(path)) else {
            return self.lower_unknown_expr(expr.syntax(), scope);
        };

        self.alloc_expr(expr.syntax(), scope, ExprKind::Path { path })
    }

    fn lower_wrapper_expr(
        &mut self,
        syntax: &rg_syntax::SyntaxNode,
        inner: Option<ast::Expr>,
        scope: ScopeId,
        kind: ExprWrapperKind,
    ) -> ExprId {
        let inner = inner.map(|inner| self.lower_expr(inner, scope));

        self.alloc_expr(syntax, scope, ExprKind::Wrapper { kind, inner })
    }

    fn lower_passthrough_unknown(
        &mut self,
        syntax: &rg_syntax::SyntaxNode,
        children: Vec<ast::Expr>,
        scope: ScopeId,
    ) -> ExprId {
        let children = children
            .into_iter()
            .map(|child| self.lower_expr(child, scope))
            .collect();

        self.alloc_expr(syntax, scope, ExprKind::Unknown { children })
    }

    fn lower_unknown_with_direct_children(
        &mut self,
        syntax: &rg_syntax::SyntaxNode,
        scope: ScopeId,
    ) -> ExprId {
        let children = syntax
            .children()
            .filter_map(ast::Expr::cast)
            .map(|child| self.lower_expr(child, scope))
            .collect();

        self.alloc_expr(syntax, scope, ExprKind::Unknown { children })
    }

    fn lower_unknown_expr(&mut self, syntax: &rg_syntax::SyntaxNode, scope: ScopeId) -> ExprId {
        self.alloc_expr(
            syntax,
            scope,
            ExprKind::Unknown {
                children: Vec::new(),
            },
        )
    }
}
