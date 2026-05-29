use rg_memsize::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::TargetRef;
use crate::declare_id;

pub use super::semantic::FunctionId as BodyFunctionId;
pub use super::semantic::ImplId as BodyImplId;

declare_id! {
    /// Stable identifier for one lowered function body inside a target.
    pub struct BodyId;

    /// Stable identifier of a single body item.
    /// While it's very similar to item tree ID, it does not come from
    /// item tree and thus is indexed separately.
    pub struct BodyItemId;

    /// Stable identifier for one value item declared inside a function body.
    pub struct BodyValueItemId;

    /// Stable identifier for one expression inside a body.
    pub struct ExprId;

    /// Stable identifier for one pattern inside a body.
    pub struct PatId;

    /// Stable identifier for one statement inside a body.
    pub struct StmtId;

    /// Stable identifier for one local binding inside a body.
    pub struct BindingId;

    /// Stable identifier for one lexical scope inside a body.
    pub struct ScopeId;
}

/// Stable reference to one lowered function body across the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyRef {
    pub target: TargetRef,
    pub body: BodyId,
}

/// Stable reference to one item declared inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyItemRef {
    pub body: BodyRef,
    pub item: BodyItemId,
}

/// Stable reference to one value item declared inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyValueItemRef {
    pub body: BodyRef,
    pub item: BodyValueItemId,
}

/// Stable reference to one local binding inside a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyBindingRef {
    pub body: BodyRef,
    pub binding: BindingId,
}

/// Stable reference to one impl block declared inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyImplRef {
    pub body: BodyRef,
    pub impl_id: BodyImplId,
}

/// Stable reference to one field declared on a body-local item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyFieldRef {
    pub item: BodyItemRef,
    pub index: usize,
}

/// Stable reference to one variant declared on a body-local enum item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyEnumVariantRef {
    pub item: BodyItemRef,
    pub index: usize,
}

/// Stable reference to one function-like declaration inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyFunctionRef {
    pub body: BodyRef,
    pub function: BodyFunctionId,
}

/// Stable reference to any declaration contributed by one lowered body.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum BodyDeclarationRef {
    Binding(BodyBindingRef),
    Item(BodyItemRef),
    ValueItem(BodyValueItemRef),
    Impl(BodyImplRef),
    Field(BodyFieldRef),
    EnumVariant(BodyEnumVariantRef),
    Function(BodyFunctionRef),
}

impl BodyDeclarationRef {
    pub fn body(self) -> BodyRef {
        match self {
            Self::Binding(declaration) => declaration.body,
            Self::Item(declaration) => declaration.body,
            Self::ValueItem(declaration) => declaration.body,
            Self::Impl(declaration) => declaration.body,
            Self::Field(declaration) => declaration.item.body,
            Self::EnumVariant(declaration) => declaration.item.body,
            Self::Function(declaration) => declaration.body,
        }
    }
}
