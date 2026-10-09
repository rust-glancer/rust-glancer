//! Const values and evaluation of literal integer expressions.
//!
//! Array lengths and const generic arguments share these rules. Evaluation uses the compilation
//! target's integer widths and keeps an expression unknown when it cannot be fully evaluated.

use std::fmt;

use rg_ir_model::{ConstParamRef, ExprBinaryOp, ExprUnaryOp, PrimitiveTy, UnsignedIntTy};
use rg_item_tree::ConstExprData;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

#[cfg(test)]
mod tests;

/// Const value retained by the semantic type model.
///
/// Literal `usize` expressions can supply array lengths and const arguments. Named consts and
/// expressions outside that subset remain unknown; generic parameters keep their identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ConstValue {
    Scalar(u128),
    Param(ConstParamRef),
    Unknown,
}

impl ConstValue {
    pub fn from_syntax(text: &str, pointer_width: Option<u32>) -> Self {
        let Some(width @ 1..=128) = pointer_width else {
            return Self::Unknown;
        };
        ConstExprData::parse(text)
            .and_then(|expr| {
                Self::evaluate(&expr, PrimitiveTy::UnsignedInt(UnsignedIntTy::Usize), width)
            })
            .map(Self::Scalar)
            .unwrap_or(Self::Unknown)
    }

    fn evaluate(expr: &ConstExprData, ty: PrimitiveTy, pointer_width: u32) -> Option<u128> {
        // Most operands inherit usize from the array length or const argument. Shift counts have
        // their own integer type, so their overflow checks and bit masks can use a different width.
        let width = ty.integer_bit_width(pointer_width)?;
        let mask = u128::MAX >> (128 - width);
        let value = match expr {
            ConstExprData::Integer { value, suffix } => {
                if suffix.is_some_and(|suffix| suffix != ty) {
                    return None;
                }
                *value
            }
            ConstExprData::Unary { op, expr } => {
                let value = Self::evaluate(expr, ty, pointer_width)?;
                match op {
                    // Complement only the bits present in the operand's type. The u128 used
                    // for storage must not turn `!0usize` into a 128-bit value on a 32-bit target.
                    ExprUnaryOp::Not => !value & mask,
                    _ => return None,
                }
            }
            ConstExprData::Binary { op, lhs, rhs } => {
                let lhs = Self::evaluate(lhs, ty, pointer_width)?;
                let rhs_ty = match op {
                    // In `1usize << (2 + 2u8)`, the count is u8. An unsuffixed count defaults
                    // to i32; borrowing usize from the left operand would change its meaning.
                    ExprBinaryOp::Shl | ExprBinaryOp::Shr => {
                        Self::explicit_integer_ty(rhs).unwrap_or(PrimitiveTy::DEFAULT_INT)
                    }
                    _ => ty,
                };
                let rhs = Self::evaluate(rhs, rhs_ty, pointer_width)?;
                Self::evaluate_binary(*op, lhs, rhs, width, mask)?
            }
        };

        // Check every operand and intermediate result, not just the final value. For example,
        // `(usize::MAX + 1) - 1` must not become a known length.
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
        // Operands have already been checked against their types. Arithmetic detects overflow
        // in u128 here; the caller also checks that the result fits the expression's integer type.
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

    fn explicit_integer_ty(expr: &ConstExprData) -> Option<PrimitiveTy> {
        // A suffix constrains both sides of arithmetic and bitwise operators. A shift only
        // inherits its left operand's type; its count is inferred separately. Evaluation checks
        // the remaining literals against this hint, so conflicting suffixes still remain unsolved.
        match expr {
            ConstExprData::Integer { suffix, .. } => *suffix,
            ConstExprData::Unary { expr, .. } => Self::explicit_integer_ty(expr),
            ConstExprData::Binary {
                op: ExprBinaryOp::Shl | ExprBinaryOp::Shr,
                lhs,
                ..
            } => Self::explicit_integer_ty(lhs),
            ConstExprData::Binary { lhs, rhs, .. } => {
                Self::explicit_integer_ty(lhs).or_else(|| Self::explicit_integer_ty(rhs))
            }
        }
    }
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(value) => value.fmt(f),
            Self::Param(_) | Self::Unknown => f.write_str("_"),
        }
    }
}
