//! Read the small part of const syntax that can be evaluated without looking up declarations.

use rg_ir_model::{ExprBinaryOp, ExprUnaryOp, PrimitiveTy};
use rg_syntax::{
    AstNode as _, Edition,
    ast::{self, HasAttrs as _},
};

use super::MaybeFromAst as _;

/// Temporary syntax for literal integer expressions. Type lowering decides which integer types and
/// results it can represent; no syntax tree or expression data is retained in the saved types.
#[derive(Debug)]
pub enum ConstExprData {
    Integer {
        value: u128,
        suffix: Option<PrimitiveTy>,
    },
    Unary {
        op: ExprUnaryOp,
        expr: Box<Self>,
    },
    Binary {
        op: ExprBinaryOp,
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
}

impl ConstExprData {
    /// Parse the complete expression before accepting any part of it. Reading a numeric prefix
    /// would turn `2 + SIZE` into the incorrect answer `2`.
    pub fn parse(text: &str) -> Option<Self> {
        // This evaluator handles short calculations, not arbitrary generated programs. Bound
        // parsing as well as the recursive walk, since editor text is not necessarily complete.
        if text.len() > 4096 {
            return None;
        }
        let parsed = ast::Expr::parse(text, Edition::CURRENT);
        if !parsed.errors().is_empty() {
            return None;
        }
        let expr = ast::Expr::cast(parsed.syntax_node())?;
        Self::from_ast(expr, 0, &mut 256)
    }

    fn from_ast(expr: ast::Expr, depth: usize, remaining: &mut usize) -> Option<Self> {
        if depth >= 64 || *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        // Attributes can change which expression exists. Leave their meaning to the ordinary
        // lowering pipeline instead of ignoring them while computing a value.
        if expr.attrs().next().is_some() {
            return None;
        }

        match expr {
            ast::Expr::Literal(literal) => {
                let ast::LiteralKind::IntNumber(number) = literal.kind() else {
                    return None;
                };
                let suffix = match number.suffix() {
                    Some(suffix) => Some(PrimitiveTy::from_integer_suffix(Some(suffix))?),
                    None => None,
                };
                Some(Self::Integer {
                    value: number.value().ok()?,
                    suffix,
                })
            }
            ast::Expr::ParenExpr(paren) => Self::from_ast(paren.expr()?, depth + 1, remaining),
            ast::Expr::BlockExpr(block) if block.modifier().is_none() => {
                // Generic arguments use braces, as in `Buffer<{ 2 + 2 }>`. Only a tail
                // expression is transparent: `{ work(); 4 }` must not silently become `4`.
                let statements = block.stmt_list()?;
                if statements.statements().next().is_some() || statements.attrs().next().is_some() {
                    return None;
                }
                Self::from_ast(statements.tail_expr()?, depth + 1, remaining)
            }
            ast::Expr::PrefixExpr(prefix) => {
                let ast::UnaryOp::Not = prefix.op_kind()? else {
                    return None;
                };
                Some(Self::Unary {
                    op: ExprUnaryOp::Not,
                    expr: Box::new(Self::from_ast(prefix.expr()?, depth + 1, remaining)?),
                })
            }
            ast::Expr::BinExpr(binary) => {
                // Comparisons and logical operators do not produce integer lengths.
                let op @ ast::BinaryOp::ArithOp(_) = binary.op_kind()? else {
                    return None;
                };
                Some(Self::Binary {
                    op: ExprBinaryOp::maybe_from_ast(&op, ())?,
                    lhs: Box::new(Self::from_ast(binary.lhs()?, depth + 1, remaining)?),
                    rhs: Box::new(Self::from_ast(binary.rhs()?, depth + 1, remaining)?),
                })
            }
            // TODO: Resolve named const initializers before extending evaluation to paths.
            // Calls, casts, and other expressions need semantics beyond literal integer operations.
            _ => None,
        }
    }
}
