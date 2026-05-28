use rg_memsize::MemorySize;
use rg_parse::TargetId;
use rg_workspace::PackageSlot;
use wincode::{SchemaRead, SchemaWrite};

use crate::declare_id;

declare_id! {
    /// Stable identifier of one module inside a target map.
    pub struct ModuleId;

    /// Stable identifier of one local definition inside a target map.
    pub struct LocalDefId;

    /// Stable identifier of one impl block inside a target map.
    pub struct LocalImplId;

    /// Stable identifier of one lowered import inside a target map.
    pub struct ImportId;
}

/// Stable reference to one target across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TargetRef {
    pub package: PackageSlot,
    pub target: TargetId,
}

/// Stable reference to one module across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ModuleRef {
    pub target: TargetRef,
    pub module: ModuleId,
}

/// Stable reference to one local definition across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct LocalDefRef {
    pub target: TargetRef,
    pub local_def: LocalDefId,
}

/// Stable reference to one impl block across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct LocalImplRef {
    pub target: TargetRef,
    pub local_impl: LocalImplId,
}

/// Stable reference to one import across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ImportRef {
    pub target: TargetRef,
    pub import: ImportId,
}

/// Namespace-resolved target-level definition reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum DefId {
    Module(ModuleRef),
    Local(LocalDefRef),
}
