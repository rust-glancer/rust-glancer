//! Context-free syntax classification used while lowering one body.
//!
//! These conversions live with Body IR because the target enums are body-owned. Keeping them on
//! the lowering context also avoids teaching item-tree about body vocabulary merely to host trait
//! implementations.

use rg_ir_model::{ExprBinaryOp, ExprUnaryOp};
use rg_syntax::ast;

use crate::ir::{
    ClosureCapture, ClosureKind, ExprAssignOp, ExprRangeKind, PatBindingMode, PatRangeKind,
    RecordFieldSyntax,
};

use super::body::BodyLowering;

impl BodyLowering<'_> {
    pub(super) fn closure_capture_from_ast(closure: &ast::ClosureExpr) -> ClosureCapture {
        if closure.move_token().is_some() {
            ClosureCapture::Move
        } else {
            ClosureCapture::Inferred
        }
    }

    pub(super) fn closure_kind_from_ast(closure: &ast::ClosureExpr) -> ClosureKind {
        if closure.async_token().is_some() {
            ClosureKind::Async
        } else {
            ClosureKind::Normal
        }
    }

    pub(super) fn unary_op_from_ast(op: ast::UnaryOp) -> ExprUnaryOp {
        match op {
            ast::UnaryOp::Deref => ExprUnaryOp::Deref,
            ast::UnaryOp::Not => ExprUnaryOp::Not,
            ast::UnaryOp::Neg => ExprUnaryOp::Neg,
        }
    }

    pub(super) fn binary_op_from_ast(op: ast::BinaryOp) -> Option<ExprBinaryOp> {
        Some(match op {
            ast::BinaryOp::LogicOp(ast::LogicOp::Or) => ExprBinaryOp::LogicOr,
            ast::BinaryOp::LogicOp(ast::LogicOp::And) => ExprBinaryOp::LogicAnd,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: false }) => ExprBinaryOp::Eq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: true }) => ExprBinaryOp::NotEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: true,
            }) => ExprBinaryOp::Less,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: false,
            }) => ExprBinaryOp::LessEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: true,
            }) => ExprBinaryOp::Greater,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: false,
            }) => ExprBinaryOp::GreaterEq,
            ast::BinaryOp::ArithOp(ast::ArithOp::Add) => ExprBinaryOp::Add,
            ast::BinaryOp::ArithOp(ast::ArithOp::Mul) => ExprBinaryOp::Mul,
            ast::BinaryOp::ArithOp(ast::ArithOp::Sub) => ExprBinaryOp::Sub,
            ast::BinaryOp::ArithOp(ast::ArithOp::Div) => ExprBinaryOp::Div,
            ast::BinaryOp::ArithOp(ast::ArithOp::Rem) => ExprBinaryOp::Rem,
            ast::BinaryOp::ArithOp(ast::ArithOp::Shl) => ExprBinaryOp::Shl,
            ast::BinaryOp::ArithOp(ast::ArithOp::Shr) => ExprBinaryOp::Shr,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitXor) => ExprBinaryOp::BitXor,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitOr) => ExprBinaryOp::BitOr,
            ast::BinaryOp::ArithOp(ast::ArithOp::BitAnd) => ExprBinaryOp::BitAnd,
            ast::BinaryOp::Assignment { .. } => return None,
        })
    }

    pub(super) fn assignment_op_from_ast(op: ast::BinaryOp) -> Option<ExprAssignOp> {
        match op {
            ast::BinaryOp::Assignment { op } => Some(match op {
                None => ExprAssignOp::Assign,
                Some(ast::ArithOp::Add) => ExprAssignOp::Add,
                Some(ast::ArithOp::Mul) => ExprAssignOp::Mul,
                Some(ast::ArithOp::Sub) => ExprAssignOp::Sub,
                Some(ast::ArithOp::Div) => ExprAssignOp::Div,
                Some(ast::ArithOp::Rem) => ExprAssignOp::Rem,
                Some(ast::ArithOp::Shl) => ExprAssignOp::Shl,
                Some(ast::ArithOp::Shr) => ExprAssignOp::Shr,
                Some(ast::ArithOp::BitXor) => ExprAssignOp::BitXor,
                Some(ast::ArithOp::BitOr) => ExprAssignOp::BitOr,
                Some(ast::ArithOp::BitAnd) => ExprAssignOp::BitAnd,
            }),
            ast::BinaryOp::LogicOp(_) | ast::BinaryOp::ArithOp(_) | ast::BinaryOp::CmpOp(_) => None,
        }
    }

    pub(super) fn expr_range_kind_from_ast(op: ast::RangeOp) -> ExprRangeKind {
        match op {
            ast::RangeOp::Exclusive => ExprRangeKind::Exclusive,
            ast::RangeOp::Inclusive => ExprRangeKind::Inclusive,
        }
    }

    pub(super) fn pat_binding_mode_from_ast(pat: &ast::IdentPat) -> PatBindingMode {
        PatBindingMode {
            by_ref: pat.ref_token().is_some(),
            mutable: pat.mut_token().is_some(),
        }
    }

    pub(super) fn pat_range_kind_from_ast(op: ast::RangeOp) -> PatRangeKind {
        match op {
            ast::RangeOp::Exclusive => PatRangeKind::Exclusive,
            ast::RangeOp::Inclusive => PatRangeKind::Inclusive,
        }
    }

    pub(super) fn record_expr_field_syntax(field: &ast::RecordExprField) -> RecordFieldSyntax {
        if field.colon_token().is_some() {
            RecordFieldSyntax::Explicit
        } else {
            RecordFieldSyntax::Shorthand
        }
    }

    pub(super) fn record_pat_field_syntax(field: &ast::RecordPatField) -> RecordFieldSyntax {
        if field.colon_token().is_some() {
            RecordFieldSyntax::Explicit
        } else {
            RecordFieldSyntax::Shorthand
        }
    }
}
