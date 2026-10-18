use std::fmt;

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::UnsignedIntTy;

/// A concrete value that can outlive the expression used to compute it.
///
/// For `const N: usize = 2 + 2`, this keeps `4` and its `usize` type. It retains no expression ids
/// or inference state, so a declaration can keep the result after its initializer body is dropped.
/// Missing results and symbolic generic parameters belong to the consuming model instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum EvaluatedConstValue {
    UnsignedInt { value: u128, ty: UnsignedIntTy },
}

impl fmt::Display for EvaluatedConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsignedInt { value, .. } => value.fmt(f),
        }
    }
}
