use wincode::{SchemaRead, SchemaWrite};

use crate::{ConstRef, FunctionRef, StaticRef, identity::DeclarationRef};
use rg_memsize::MemorySize;

/// Semantic item that owns a lowered expression body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum BodyOwner {
    /// Function body, such as `fn read() { value }`.
    Function(FunctionRef),
    /// Const initializer body, such as `const LIMIT: u8 = value;`.
    Const(ConstRef),
    /// Static initializer body, such as `static CURRENT: u8 = value;`.
    Static(StaticRef),
}

impl BodyOwner {
    /// Returns the function ref when this body is owned by a function declaration.
    pub fn function(self) -> Option<FunctionRef> {
        match self {
            Self::Function(function) => Some(function),
            Self::Const(_) | Self::Static(_) => None,
        }
    }

    /// Returns the declaration that should own facts derived from this body.
    pub fn declaration(self) -> DeclarationRef {
        match self {
            Self::Function(function) => DeclarationRef::from(function),
            Self::Const(const_ref) => DeclarationRef::from(const_ref),
            Self::Static(static_ref) => DeclarationRef::from(static_ref),
        }
    }
}
