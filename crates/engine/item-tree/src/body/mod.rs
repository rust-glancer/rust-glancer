//! Context-free AST conversions for lowered body vocabulary.
//!
//! Body IR still owns recursive lowering because it needs scopes, bindings, and body-local state.
//! These impls only translate syntax spelling into model enums and small value objects.

use rg_ir_model::{
    ClosureCapture, ClosureKind, ExprAssignOp, ExprBinaryOp, ExprRangeKind, ExprUnaryOp,
    PatBindingMode, PatRangeKind, RecordFieldSyntax,
};
use rg_syntax::ast;

use crate::item::{FromAst, MaybeFromAst};

pub struct RecordExprFieldAst;
pub struct RecordPatFieldAst;

impl FromAst for ClosureCapture {
    type AstNode = ast::ClosureExpr;
    type Context<'a> = ();

    fn from_ast(closure: &Self::AstNode, (): Self::Context<'_>) -> Self {
        if closure.move_token().is_some() {
            Self::Move
        } else {
            Self::Inferred
        }
    }
}

impl FromAst for ClosureKind {
    type AstNode = ast::ClosureExpr;
    type Context<'a> = ();

    fn from_ast(closure: &Self::AstNode, (): Self::Context<'_>) -> Self {
        if closure.async_token().is_some() {
            Self::Async
        } else {
            Self::Normal
        }
    }
}

impl FromAst for ExprUnaryOp {
    type AstNode = ast::UnaryOp;
    type Context<'a> = ();

    fn from_ast(op: &Self::AstNode, (): Self::Context<'_>) -> Self {
        match *op {
            ast::UnaryOp::Deref => Self::Deref,
            ast::UnaryOp::Not => Self::Not,
            ast::UnaryOp::Neg => Self::Neg,
        }
    }
}

impl MaybeFromAst for ExprBinaryOp {
    type AstNode = ast::BinaryOp;
    type Context<'a> = ();

    fn maybe_from_ast(op: &Self::AstNode, (): Self::Context<'_>) -> Option<Self> {
        Some(match *op {
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
            ast::BinaryOp::Assignment { .. } => return None,
        })
    }
}

impl MaybeFromAst for ExprAssignOp {
    type AstNode = ast::BinaryOp;
    type Context<'a> = ();

    fn maybe_from_ast(op: &Self::AstNode, (): Self::Context<'_>) -> Option<Self> {
        match *op {
            ast::BinaryOp::Assignment { op } => Some(match op {
                None => Self::Assign,
                Some(ast::ArithOp::Add) => Self::Add,
                Some(ast::ArithOp::Mul) => Self::Mul,
                Some(ast::ArithOp::Sub) => Self::Sub,
                Some(ast::ArithOp::Div) => Self::Div,
                Some(ast::ArithOp::Rem) => Self::Rem,
                Some(ast::ArithOp::Shl) => Self::Shl,
                Some(ast::ArithOp::Shr) => Self::Shr,
                Some(ast::ArithOp::BitXor) => Self::BitXor,
                Some(ast::ArithOp::BitOr) => Self::BitOr,
                Some(ast::ArithOp::BitAnd) => Self::BitAnd,
            }),
            ast::BinaryOp::LogicOp(_) | ast::BinaryOp::ArithOp(_) | ast::BinaryOp::CmpOp(_) => None,
        }
    }
}

impl FromAst for ExprRangeKind {
    type AstNode = ast::RangeOp;
    type Context<'a> = ();

    fn from_ast(op: &Self::AstNode, (): Self::Context<'_>) -> Self {
        match *op {
            ast::RangeOp::Exclusive => Self::Exclusive,
            ast::RangeOp::Inclusive => Self::Inclusive,
        }
    }
}

impl FromAst for PatBindingMode {
    type AstNode = ast::IdentPat;
    type Context<'a> = ();

    fn from_ast(pat: &Self::AstNode, (): Self::Context<'_>) -> Self {
        Self {
            by_ref: pat.ref_token().is_some(),
            mutable: pat.mut_token().is_some(),
        }
    }
}

impl FromAst for PatRangeKind {
    type AstNode = ast::RangeOp;
    type Context<'a> = ();

    fn from_ast(op: &Self::AstNode, (): Self::Context<'_>) -> Self {
        match *op {
            ast::RangeOp::Exclusive => Self::Exclusive,
            ast::RangeOp::Inclusive => Self::Inclusive,
        }
    }
}

impl FromAst<RecordExprFieldAst> for RecordFieldSyntax {
    type AstNode = ast::RecordExprField;
    type Context<'a> = RecordExprFieldAst;

    fn from_ast(field: &Self::AstNode, _ctx: Self::Context<'_>) -> Self {
        if field.colon_token().is_some() {
            Self::Explicit
        } else {
            Self::Shorthand
        }
    }
}

impl FromAst<RecordPatFieldAst> for RecordFieldSyntax {
    type AstNode = ast::RecordPatField;
    type Context<'a> = RecordPatFieldAst;

    fn from_ast(field: &Self::AstNode, _ctx: Self::Context<'_>) -> Self {
        if field.colon_token().is_some() {
            Self::Explicit
        } else {
            Self::Shorthand
        }
    }
}
