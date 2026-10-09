//! Convert expression operators into the shared IR vocabulary.

use rg_ir_model::ExprBinaryOp;
use rg_syntax::ast;

use super::MaybeFromAst;

impl MaybeFromAst for ExprBinaryOp {
    type AstNode = ast::BinaryOp;
    type Context<'a> = ();

    fn maybe_from_ast(node: &Self::AstNode, (): Self::Context<'_>) -> Option<Self> {
        Some(match node {
            ast::BinaryOp::LogicOp(ast::LogicOp::Or) => Self::LogicOr,
            ast::BinaryOp::LogicOp(ast::LogicOp::And) => Self::LogicAnd,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: false }) => Self::Eq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: true }) => Self::NotEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: true,
            }) => Self::Less,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: false,
            }) => Self::LessEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: true,
            }) => Self::Greater,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: false,
            }) => Self::GreaterEq,
            ast::BinaryOp::ArithOp(ast::ArithOp::Add) => Self::Add,
            ast::BinaryOp::ArithOp(ast::ArithOp::Mul) => Self::Mul,
            ast::BinaryOp::ArithOp(ast::ArithOp::Sub) => Self::Sub,
            ast::BinaryOp::ArithOp(ast::ArithOp::Div) => Self::Div,
            ast::BinaryOp::ArithOp(ast::ArithOp::Rem) => Self::Rem,
            ast::BinaryOp::ArithOp(ast::ArithOp::Shl) => Self::Shl,
            ast::BinaryOp::ArithOp(ast::ArithOp::Shr) => Self::Shr,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitXor) => Self::BitXor,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitOr) => Self::BitOr,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitAnd) => Self::BitAnd,
            // Assignment has its own IR node and operator vocabulary.
            ast::BinaryOp::Assignment { .. } => return None,
        })
    }
}
