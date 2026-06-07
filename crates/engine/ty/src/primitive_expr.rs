use rg_ir_model::{ExprBinaryOp, ExprUnaryOp, LiteralKind};

use crate::{PrimitiveTy, RefMutability, Ty};

/// Returns the primitive type implied by a lowered literal token.
pub fn ty_for_literal(kind: LiteralKind) -> Ty {
    match kind {
        LiteralKind::Bool => Ty::Primitive(PrimitiveTy::Bool),
        LiteralKind::Char => Ty::Primitive(PrimitiveTy::Char),
        LiteralKind::Float { primitive_ty } | LiteralKind::Int { primitive_ty } => {
            primitive_ty.map(Ty::Primitive).unwrap_or(Ty::Unknown)
        }
        LiteralKind::String => {
            Ty::reference(RefMutability::Shared, Ty::Primitive(PrimitiveTy::Str))
        }
        LiteralKind::Unknown => Ty::Unknown,
    }
}

/// Returns the local primitive result type for a unary expression.
pub fn ty_for_unary(op: ExprUnaryOp, inner: &Ty) -> Ty {
    match op {
        ExprUnaryOp::Not => match inner {
            Ty::Primitive(primitive) if primitive.is_bool() || primitive.is_integral() => {
                Ty::Primitive(*primitive)
            }
            _ => Ty::Unknown,
        },
        ExprUnaryOp::Neg => match inner {
            Ty::Primitive(primitive) if primitive.is_signed_numeric() => Ty::Primitive(*primitive),
            _ => Ty::Unknown,
        },
        ExprUnaryOp::Deref => Ty::Unknown,
    }
}

/// Returns the local primitive result type for a binary expression.
pub fn ty_for_binary(op: ExprBinaryOp, lhs: &Ty, rhs: &Ty) -> Ty {
    if op.is_logical() || op.is_comparison() {
        return Ty::Primitive(PrimitiveTy::Bool);
    }

    match op {
        ExprBinaryOp::Add
        | ExprBinaryOp::Sub
        | ExprBinaryOp::Mul
        | ExprBinaryOp::Div
        | ExprBinaryOp::Rem => symmetric_primitive_op_ty(lhs, rhs, |ty| ty.is_numeric()),
        ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor => {
            symmetric_primitive_op_ty(lhs, rhs, |ty| ty.is_integral() || ty.is_bool())
        }
        ExprBinaryOp::Shl | ExprBinaryOp::Shr => shift_op_ty(lhs, rhs),
        ExprBinaryOp::LogicOr
        | ExprBinaryOp::LogicAnd
        | ExprBinaryOp::Eq
        | ExprBinaryOp::NotEq
        | ExprBinaryOp::Less
        | ExprBinaryOp::LessEq
        | ExprBinaryOp::Greater
        | ExprBinaryOp::GreaterEq => Ty::Primitive(PrimitiveTy::Bool),
    }
}

fn symmetric_primitive_op_ty(
    lhs_ty: &Ty,
    rhs_ty: &Ty,
    accepts: impl Fn(PrimitiveTy) -> bool,
) -> Ty {
    match (lhs_ty, rhs_ty) {
        (Ty::Primitive(lhs), Ty::Primitive(rhs)) if lhs == rhs && accepts(*lhs) => {
            Ty::Primitive(*lhs)
        }
        (Ty::Primitive(lhs), Ty::Unknown) if accepts(*lhs) => Ty::Primitive(*lhs),
        (Ty::Unknown, Ty::Primitive(rhs)) if accepts(*rhs) => Ty::Primitive(*rhs),
        _ => Ty::Unknown,
    }
}

fn shift_op_ty(lhs_ty: &Ty, rhs_ty: &Ty) -> Ty {
    match (lhs_ty, rhs_ty) {
        (Ty::Primitive(lhs), Ty::Primitive(rhs)) if lhs.is_integral() && rhs.is_integral() => {
            Ty::Primitive(*lhs)
        }
        (Ty::Primitive(lhs), Ty::Unknown) if lhs.is_integral() => Ty::Primitive(*lhs),
        _ => Ty::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::items::{FloatTy, SignedIntTy, UnsignedIntTy};

    use super::*;

    fn int() -> Ty {
        Ty::Primitive(PrimitiveTy::SignedInt(SignedIntTy::I32))
    }

    fn uint() -> Ty {
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U32))
    }

    fn float() -> Ty {
        Ty::Primitive(PrimitiveTy::Float(FloatTy::F32))
    }

    fn bool_ty() -> Ty {
        Ty::Primitive(PrimitiveTy::Bool)
    }

    #[test]
    fn types_literals_from_lowered_literal_kinds() {
        let cases = [
            (LiteralKind::Bool, bool_ty(), "bool literal"),
            (
                LiteralKind::Char,
                Ty::Primitive(PrimitiveTy::Char),
                "char literal",
            ),
            (
                LiteralKind::Int {
                    primitive_ty: Some(PrimitiveTy::SignedInt(SignedIntTy::I32)),
                },
                int(),
                "integer literal",
            ),
            (
                LiteralKind::Float {
                    primitive_ty: Some(PrimitiveTy::Float(FloatTy::F32)),
                },
                float(),
                "float literal",
            ),
            (
                LiteralKind::String,
                Ty::reference(RefMutability::Shared, Ty::Primitive(PrimitiveTy::Str)),
                "string literal",
            ),
            (
                LiteralKind::Int { primitive_ty: None },
                Ty::Unknown,
                "unknown integer literal",
            ),
            (LiteralKind::Unknown, Ty::Unknown, "unknown literal"),
        ];

        for (kind, expected, label) in cases {
            assert_eq!(ty_for_literal(kind), expected, "{label}");
        }
    }

    #[test]
    fn types_unary_primitive_operators() {
        let cases = [
            (ExprUnaryOp::Not, bool_ty(), bool_ty(), "bool not"),
            (ExprUnaryOp::Not, int(), int(), "integer not"),
            (ExprUnaryOp::Not, float(), Ty::Unknown, "float not"),
            (ExprUnaryOp::Neg, int(), int(), "signed neg"),
            (ExprUnaryOp::Neg, uint(), Ty::Unknown, "unsigned neg"),
            (ExprUnaryOp::Neg, float(), float(), "float neg"),
            (ExprUnaryOp::Deref, int(), Ty::Unknown, "deref"),
        ];

        for (op, inner, expected, label) in cases {
            assert_eq!(ty_for_unary(op, &inner), expected, "{label}");
        }
    }

    #[test]
    fn types_binary_primitive_operators() {
        let cases = [
            (ExprBinaryOp::Add, int(), int(), int(), "integer add"),
            (ExprBinaryOp::Sub, float(), float(), float(), "float sub"),
            (
                ExprBinaryOp::Mul,
                int(),
                Ty::Unknown,
                int(),
                "partial numeric",
            ),
            (
                ExprBinaryOp::Add,
                int(),
                uint(),
                Ty::Unknown,
                "mixed numeric",
            ),
            (
                ExprBinaryOp::BitAnd,
                bool_ty(),
                bool_ty(),
                bool_ty(),
                "bool bitand",
            ),
            (ExprBinaryOp::BitOr, int(), int(), int(), "integer bitor"),
            (
                ExprBinaryOp::BitXor,
                float(),
                float(),
                Ty::Unknown,
                "float bitxor",
            ),
            (ExprBinaryOp::Shl, int(), uint(), int(), "integer shift"),
            (
                ExprBinaryOp::Shr,
                int(),
                Ty::Unknown,
                int(),
                "partial shift",
            ),
            (ExprBinaryOp::Eq, int(), float(), bool_ty(), "comparison"),
            (ExprBinaryOp::LogicAnd, int(), float(), bool_ty(), "logical"),
        ];

        for (op, lhs, rhs, expected, label) in cases {
            assert_eq!(ty_for_binary(op, &lhs, &rhs), expected, "{label}");
        }
    }
}
