//! Const expression evaluation shared by declarations, array lengths, and const arguments.
//!
//! Frontends expose their expression nodes through `ConstExprSource`; this module owns the walk
//! and the compilation target's arithmetic rules. It returns a concrete value only when the whole
//! expression can be evaluated.
//!
//! Named declarations retain concrete results in their ItemStore. Types use `ConstValue` below,
//! which can also preserve a generic parameter's identity, as in `[u8; N]`.

use std::fmt;

use rg_ir_model::{ConstParamRef, EvaluatedConstValue, PrimitiveTy, UnsignedIntTy};
use rg_item_tree::ConstExprData;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

mod query;
mod source;

pub use self::{
    query::ConstEvaluationQuery,
    source::{ConstExprSource, ConstExprView},
};

#[cfg(test)]
mod tests;

/// Const value retained by the semantic type model.
///
/// Literal `usize` expressions can supply array lengths and const arguments. References to const
/// items in types remain unknown, as do unsupported expressions; generic parameters keep their identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ConstValue {
    Scalar(u128),
    Param(ConstParamRef),
    Unknown,
}

impl ConstValue {
    pub fn from_syntax(text: &str, pointer_width: Option<u32>) -> Self {
        let value = ConstExprData::parse(text).and_then(|expr| {
            let source = &expr;
            ConstEvaluationQuery::new(&source, pointer_width)
                .evaluate_expr(source, PrimitiveTy::UnsignedInt(UnsignedIntTy::Usize))
        });
        match value {
            Some(EvaluatedConstValue::UnsignedInt {
                value,
                ty: UnsignedIntTy::Usize,
            }) => Self::Scalar(value),
            _ => Self::Unknown,
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
