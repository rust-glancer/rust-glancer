use rg_parse::TargetId;
use rg_workspace::PackageSlot;

pub use rg_ir_model::{
    DefId, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
    ModuleRef, TargetRef,
};

/// Target reference proven to come from a resident phase-DB package entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResidentTargetRef {
    pub package: PackageSlot,
    pub target: TargetId,
}
