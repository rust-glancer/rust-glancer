//! Context-free syntax classification used while lowering one body.
//!
//! These conversions are only needed while lowering bodies. Keeping them on the lowering context
//! also avoids teaching item-tree about body vocabulary merely to host trait implementations.

use rg_ir_model::ExprUnaryOp;
use rg_syntax::ast;

use super::body::BodyLowering;
use crate::body::{
    ClosureCapture, ClosureKind, ExprAssignOp, ExprRangeKind, PatBindingMode, PatRangeKind,
    RecordFieldSyntax,
};

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
