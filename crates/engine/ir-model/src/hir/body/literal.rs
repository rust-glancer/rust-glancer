use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use crate::items::PrimitiveTy;
use rg_std::{MemorySize, Shrink};

/// Literal category plus the primitive type implied by suffix/default heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum LiteralKind {
    Bool,
    Char,
    Float { primitive_ty: Option<PrimitiveTy> },
    Int { primitive_ty: Option<PrimitiveTy> },
    String,
    Unknown,
}

impl fmt::Display for LiteralKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::Char => write!(f, "char"),
            Self::Float { .. } => write!(f, "float"),
            Self::Int { .. } => write!(f, "int"),
            Self::String => write!(f, "string"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}
