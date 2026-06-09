use rg_parse::TargetId;
use rg_workspace::PackageSlot;
use wincode::{SchemaRead, SchemaWrite};

use crate::{BodyRef, declare_id};
use rg_std::{MemorySize, Shrink};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct TargetRef {
    pub package: PackageSlot,
    pub target: TargetId,
}

/// Stable reference to one def map item.
// Note: we intentionally do not derive or provide `From` here, as it can be very
// easy to just convert `TargetRef` (which is always present) where `BodyRef` must
// be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum DefMapRef {
    /// Item originates from a target (e.g. semantic scope)
    Target(TargetRef),
    /// Item originates from a certain function body (e.g. body scope)
    Body(BodyRef),
}

impl DefMapRef {
    /// If `DefMapRef` originated from a target, returns the corresponding
    /// target ref.
    pub fn as_target_ref(&self) -> Option<TargetRef> {
        match self {
            Self::Target(target) => Some(*target),
            Self::Body(_) => None,
        }
    }

    /// Returns the target that contains the object identified by this ref,
    /// regardless whether the object originates in target or in body.
    ///
    /// This method must not be confused with `as_target_ref`.
    pub fn origin_target(&self) -> TargetRef {
        match self {
            Self::Target(target) => *target,
            Self::Body(body) => body.target,
        }
    }
}

/// Stable reference to one module across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ModuleRef {
    pub origin: DefMapRef,
    pub module: ModuleId,
}

/// Stable reference to one local definition across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LocalDefRef {
    pub origin: DefMapRef,
    pub local_def: LocalDefId,
}

/// Stable reference to one impl block across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LocalImplRef {
    pub origin: DefMapRef,
    pub local_impl: LocalImplId,
}

/// Stable reference to one import across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ImportRef {
    pub origin: DefMapRef,
    pub import: ImportId,
}

/// Namespace-resolved target-level definition reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum DefId {
    Module(ModuleRef),
    Local(LocalDefRef),
}
