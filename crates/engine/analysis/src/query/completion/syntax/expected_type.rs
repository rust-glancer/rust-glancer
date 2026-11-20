//! Find where the surrounding code tells us what type to expect.
//!
//! At `let value: User = $0`, the completion marker gives us an initializer to walk up
//! from. The annotation belongs to the editor's source, so we map the statement back
//! to that source before asking Body IR for its type.

use rg_ir_model::Span;
use rg_syntax::{
    AstNode as _, SyntaxKind, SyntaxNode,
    ast::{self, BlockModifier, HasGenericArgs as _},
};

use super::CompletionSyntaxContext;

/// The part of the original source to query for the expected type.
pub(crate) enum ExpectedTypeSite {
    Argument { call_start: u32 },
    Let { statement_start: u32 },
    Assignment { target: Span },
    FunctionReturn,
}

/// The syntax around the name controls what a candidate produces. For example, completing
/// `Event::Re$0(1)` supplies a tuple constructor call, while `Event::Re$0` supplies a bare path.
#[derive(Clone, Copy)]
pub(crate) enum ValueExpression {
    Path,
    Call,
    Record,
}

pub(crate) struct ExpectedTypeContext {
    pub(crate) site: ExpectedTypeSite,
    pub(crate) expression: ValueExpression,
    /// Arguments on the name being completed, as in `Po$0::<u8>(0)`.
    pub(crate) member_has_generic_args: bool,
}

impl CompletionSyntaxContext<'_> {
    pub(crate) fn expected_type_context(&self) -> Option<ExpectedTypeContext> {
        let mut expr = self.marker.parent_ancestors().find_map(ast::Expr::cast)?;
        let path = match &expr {
            ast::Expr::PathExpr(path) => path.path()?,
            ast::Expr::RecordExpr(record) => record.path()?,
            _ => return None,
        };
        // In `ta$0::new()` the candidate supplies a qualifier. In `Task::ne$0()`
        // it supplies the callable whose result the surrounding expression consumes.
        let member = path.segment()?;
        let name = member.name_ref()?;
        if !name
            .syntax()
            .text_range()
            .contains_range(self.marker.text_range())
        {
            return None;
        }

        let expression = if let Some(call) = expr.syntax().parent().and_then(ast::CallExpr::cast)
            && call.expr().as_ref() == Some(&expr)
        {
            expr = ast::Expr::CallExpr(call);
            ValueExpression::Call
        } else if matches!(expr, ast::Expr::RecordExpr(_)) {
            ValueExpression::Record
        } else {
            ValueExpression::Path
        };

        Some(ExpectedTypeContext {
            site: self.expected_type_site(expr)?,
            expression,
            member_has_generic_args: member.generic_arg_list().is_some(),
        })
    }

    /// Walk from the expression to the syntax that supplies its expected type.
    fn expected_type_site(&self, mut expr: ast::Expr) -> Option<ExpectedTypeSite> {
        use SyntaxKind::{
            ARG_LIST, BIN_EXPR, FN, IF_EXPR, LET_STMT, MATCH_ARM, PAREN_EXPR, RETURN_EXPR,
            STMT_LIST,
        };

        loop {
            let parent = expr.syntax().parent()?;
            expr = match parent.kind() {
                ARG_LIST => {
                    let call = parent.parent()?;
                    return Some(ExpectedTypeSite::Argument {
                        call_start: self.original_span(call.text_range())?.start,
                    });
                }
                LET_STMT => {
                    let stmt = ast::LetStmt::cast(parent)?;
                    if stmt.ty().is_none() || stmt.initializer().as_ref() != Some(&expr) {
                        return None;
                    }
                    return Some(ExpectedTypeSite::Let {
                        statement_start: self.original_span(stmt.syntax().text_range())?.start,
                    });
                }
                BIN_EXPR => {
                    let assign = ast::BinExpr::cast(parent)?;
                    if !matches!(
                        assign.op_kind(),
                        Some(ast::BinaryOp::Assignment { op: None })
                    ) || assign.rhs().as_ref() != Some(&expr)
                    {
                        return None;
                    }
                    return Some(ExpectedTypeSite::Assignment {
                        target: self.original_span(assign.lhs()?.syntax().text_range())?,
                    });
                }
                RETURN_EXPR if Self::return_targets_function(&parent) => {
                    return Some(ExpectedTypeSite::FunctionReturn);
                }
                FN if matches!(expr, ast::Expr::BlockExpr(_)) => {
                    return Some(ExpectedTypeSite::FunctionReturn);
                }

                // These parents forward the expression's value. For example, `send((va$0))`
                // lets us use the parameter type, while `send(va$0 + 1)` stops at the operator.
                PAREN_EXPR => ast::Expr::cast(parent)?,
                STMT_LIST => {
                    let stmts = ast::StmtList::cast(parent)?;
                    if stmts.tail_expr().as_ref() != Some(&expr) {
                        return None;
                    }
                    let block = stmts.syntax().parent().and_then(ast::BlockExpr::cast)?;
                    if !matches!(
                        block.modifier(),
                        None | Some(BlockModifier::Label(_) | BlockModifier::Unsafe(_))
                    ) {
                        return None;
                    }
                    ast::Expr::BlockExpr(block)
                }
                IF_EXPR => {
                    let branch = ast::IfExpr::cast(parent)?;
                    let otherwise = branch.else_branch()?;
                    let is_result = otherwise.syntax() == expr.syntax()
                        || branch
                            .then_branch()
                            .is_some_and(|block| block.syntax() == expr.syntax());
                    if !is_result {
                        return None;
                    }
                    ast::Expr::IfExpr(branch)
                }
                MATCH_ARM => {
                    let arm = ast::MatchArm::cast(parent)?;
                    if arm.expr().as_ref() != Some(&expr) {
                        return None;
                    }
                    let branches = arm
                        .syntax()
                        .parent()?
                        .parent()
                        .and_then(ast::MatchExpr::cast)?;
                    ast::Expr::MatchExpr(branches)
                }
                _ => return None,
            };
        }
    }

    /// A closure or an async block owns its returns. Stop there before using the
    /// enclosing function's output type.
    fn return_targets_function(return_expr: &SyntaxNode) -> bool {
        for node in return_expr.ancestors() {
            match node.kind() {
                SyntaxKind::FN => return true,
                SyntaxKind::CLOSURE_EXPR | SyntaxKind::CONST | SyntaxKind::STATIC => return false,
                SyntaxKind::BLOCK_EXPR => {
                    let modifier = ast::BlockExpr::cast(node).and_then(|block| block.modifier());
                    if matches!(
                        modifier,
                        Some(
                            BlockModifier::Async(_)
                                | BlockModifier::AsyncGen(_)
                                | BlockModifier::Gen(_)
                                | BlockModifier::Const(_)
                        )
                    ) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        false
    }
}
