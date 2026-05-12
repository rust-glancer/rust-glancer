use rg_parse::TargetId;

use rg_workspace::PackageSlot;

macro_rules! impl_arena_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl rg_arena::ArenaId for $id {
                fn from_index(index: usize) -> Self {
                    Self(index)
                }

                fn index(self) -> usize {
                    self.0
                }
            }
        )+
    };
}

/// Stable identifier of one module inside a target map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ModuleId(pub usize);

/// Stable identifier of one local definition inside a target map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LocalDefId(pub usize);

/// Stable identifier of one impl block inside a target map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LocalImplId(pub usize);

/// Stable identifier of one lowered import inside a target map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ImportId(pub usize);

impl_arena_id!(ModuleId, LocalDefId, LocalImplId, ImportId);

/// Stable reference to one target across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct TargetRef {
    pub package: PackageSlot,
    pub target: TargetId,
}

/// Target reference proven to come from a resident phase-DB package entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResidentTargetRef {
    pub package: PackageSlot,
    pub target: TargetId,
}

/// Stable reference to one module across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ModuleRef {
    pub target: TargetRef,
    pub module: ModuleId,
}

/// Stable reference to one local definition across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LocalDefRef {
    pub target: TargetRef,
    pub local_def: LocalDefId,
}

/// Stable reference to one impl block across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LocalImplRef {
    pub target: TargetRef,
    pub local_impl: LocalImplId,
}

/// Stable reference to one import across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ImportRef {
    pub target: TargetRef,
    pub import: ImportId,
}

/// Namespace-resolved target-level definition reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum DefId {
    Module(ModuleRef),
    Local(LocalDefRef),
}
