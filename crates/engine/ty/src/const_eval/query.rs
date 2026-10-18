use rg_ir_model::{EvaluatedConstValue, ExprBinaryOp, ExprUnaryOp, PrimitiveTy, UnsignedIntTy};

use super::{ConstExprSource, ConstExprView};

/// Evaluate expressions without depending on the storage or lowering phase that owns them.
///
/// The caller supplies an already-resolved expected type. For `const N: Size = 2 + 2`, the body
/// layer resolves `Size` before asking this query to do arithmetic. The query follows nodes through
/// its source, so it needs neither an owned copy of the expression nor a dependency on Body IR.
pub struct ConstEvaluationQuery<'a, S> {
    source: &'a S,
    pointer_width: Option<u32>,
}

impl<'a, S: ConstExprSource> ConstEvaluationQuery<'a, S> {
    pub fn new(source: &'a S, pointer_width: Option<u32>) -> Self {
        Self {
            source,
            pointer_width,
        }
    }

    /// The expected semantic type supplies the overflow rules for unsuffixed literals.
    /// `usize` uses the compilation target's pointer width, not the machine running the query.
    /// Unsupported nodes, overflow, or an exhausted walk budget leave the whole result unknown.
    pub fn evaluate_expr(
        &self,
        root: S::Expr,
        expected_ty: PrimitiveTy,
    ) -> Option<EvaluatedConstValue> {
        // TODO: Evaluate other scalar types when their value representations are supported.
        if expected_ty != PrimitiveTy::UnsignedInt(UnsignedIntTy::Usize) {
            return None;
        }
        let pointer_width @ 1..=128 = self.pointer_width? else {
            return None;
        };
        let value = self.evaluate(root, expected_ty, pointer_width, 0, &mut 256)?;
        Some(EvaluatedConstValue::UnsignedInt {
            value,
            ty: UnsignedIntTy::Usize,
        })
    }

    fn evaluate(
        &self,
        expr: S::Expr,
        ty: PrimitiveTy,
        pointer_width: u32,
        depth: usize,
        remaining: &mut usize,
    ) -> Option<u128> {
        // Sources can contain incomplete or deeply nested editor expressions. Keep one bounded
        // evaluation walk regardless of which frontend supplied the nodes.
        if depth >= 64 || *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        let width = ty.integer_bit_width(pointer_width)?;
        let mask = u128::MAX >> (128 - width);
        let value = match self.source.expr(expr)? {
            ConstExprView::Integer { value, suffix } => {
                if suffix.is_some_and(|suffix| suffix != ty) {
                    return None;
                }
                value
            }
            ConstExprView::Transparent(inner) => {
                self.evaluate(inner, ty, pointer_width, depth + 1, remaining)?
            }
            ConstExprView::Unary { op, expr } => {
                let value = self.evaluate(expr, ty, pointer_width, depth + 1, remaining)?;
                match op {
                    // Complement only the bits present in the operand's type, rather than all
                    // 128 bits used to store it. For example, !0usize depends on the target.
                    ExprUnaryOp::Not => !value & mask,
                    _ => return None,
                }
            }
            ConstExprView::Binary { op, lhs, rhs } => {
                let lhs = self.evaluate(lhs, ty, pointer_width, depth + 1, remaining)?;
                let rhs_ty = match op {
                    // In `1usize << (2 + 2u8)`, the count is u8. An unsuffixed count defaults
                    // to i32; borrowing usize from the left operand would change its meaning.
                    ExprBinaryOp::Shl | ExprBinaryOp::Shr => self
                        .explicit_integer_ty(rhs, depth + 1, &mut 256)
                        .unwrap_or(PrimitiveTy::DEFAULT_INT),
                    _ => ty,
                };
                let rhs = self.evaluate(rhs, rhs_ty, pointer_width, depth + 1, remaining)?;
                Self::evaluate_binary(op, lhs, rhs, width, mask)?
            }
        };

        // Check each intermediate result: `(usize::MAX + 1) - 1` must not become a known value.
        // TODO: Support negative intermediate values in signed shift-count expressions.
        let max = if matches!(ty, PrimitiveTy::SignedInt(_)) {
            mask >> 1
        } else {
            mask
        };
        (value <= max).then_some(value)
    }

    fn evaluate_binary(
        op: ExprBinaryOp,
        lhs: u128,
        rhs: u128,
        width: u32,
        mask: u128,
    ) -> Option<u128> {
        // The caller checks the target type's range after these u128 operations.
        Some(match op {
            ExprBinaryOp::Add => lhs.checked_add(rhs)?,
            ExprBinaryOp::Sub => lhs.checked_sub(rhs)?,
            ExprBinaryOp::Mul => lhs.checked_mul(rhs)?,
            ExprBinaryOp::Div => lhs.checked_div(rhs)?,
            ExprBinaryOp::Rem => lhs.checked_rem(rhs)?,
            ExprBinaryOp::BitAnd => lhs & rhs,
            ExprBinaryOp::BitOr => lhs | rhs,
            ExprBinaryOp::BitXor => lhs ^ rhs,
            // Rust rejects an out-of-range count, but allows high bits to be discarded:
            // on a 32-bit target, `0x8000_0000usize << 1` is zero.
            ExprBinaryOp::Shl if rhs < u128::from(width) => (lhs << rhs) & mask,
            ExprBinaryOp::Shr if rhs < u128::from(width) => lhs >> rhs,
            _ => return None,
        })
    }

    fn explicit_integer_ty(
        &self,
        expr: S::Expr,
        depth: usize,
        remaining: &mut usize,
    ) -> Option<PrimitiveTy> {
        if depth >= 64 || *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        // A suffix constrains arithmetic operands. Shifts inherit only the left operand's type;
        // their counts are inferred separately. The evaluation walk checks conflicting suffixes.
        match self.source.expr(expr)? {
            ConstExprView::Integer { suffix, .. } => suffix,
            ConstExprView::Unary { expr, .. } | ConstExprView::Transparent(expr) => {
                self.explicit_integer_ty(expr, depth + 1, remaining)
            }
            ConstExprView::Binary {
                op: ExprBinaryOp::Shl | ExprBinaryOp::Shr,
                lhs,
                ..
            } => self.explicit_integer_ty(lhs, depth + 1, remaining),
            ConstExprView::Binary { lhs, rhs, .. } => self
                .explicit_integer_ty(lhs, depth + 1, remaining)
                .or_else(|| self.explicit_integer_ty(rhs, depth + 1, remaining)),
        }
    }
}
