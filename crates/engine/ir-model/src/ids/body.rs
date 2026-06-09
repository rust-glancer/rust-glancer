use wincode::{SchemaRead, SchemaWrite};

use crate::TargetRef;
use crate::declare_id;
use rg_std::{MemorySize, Shrink};

declare_id! {
    /// Stable identifier for one lowered body inside a target.
    pub struct BodyId;

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

/// Stable reference to one lowered body across the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyRef {
    pub target: TargetRef,
    pub body: BodyId,
}

/// Stable reference to one local binding inside a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyBindingRef {
    pub body: BodyRef,
    pub binding: BindingId,
}
