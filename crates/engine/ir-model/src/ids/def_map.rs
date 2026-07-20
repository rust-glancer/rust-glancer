use rg_workspace::PackageSlot;
use wincode::{SchemaRead, SchemaWrite};

use crate::{BodyRef, declare_id};
use rg_std::{MemorySize, Shrink};

declare_id! {
    /// Package-local identifier of one semantic crate.
    pub struct CrateId;

    /// Stable identifier of one module inside a crate map.
    pub struct ModuleId;

    /// Stable identifier of one local definition inside a crate map.
    pub struct LocalDefId;

    /// Stable identifier of one enum variant inside a crate map.
    pub struct LocalEnumVariantId;

    /// Stable identifier of one impl block inside a crate map.
    pub struct LocalImplId;

    /// Stable identifier of one lowered import inside a crate map.
    pub struct ImportId;
}

/// Stable reference to one semantic crate across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct CrateRef {
    pub package: PackageSlot,
    pub crate_id: CrateId,
}

/// Stable reference to one def map item.
// Note: we intentionally do not derive or provide `From` here, as it can be very
// easy to just convert `CrateRef` (which is always present) where `BodyRef` must
// be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum DefMapRef {
    /// Item originates from a crate (e.g. semantic scope).
    Crate(CrateRef),
    /// Item originates from a certain function body (e.g. body scope)
    Body(BodyRef),
}

impl DefMapRef {
    /// If `DefMapRef` originated from a crate, returns the corresponding crate ref.
    pub fn as_crate_ref(&self) -> Option<CrateRef> {
        match self {
            Self::Crate(crate_ref) => Some(*crate_ref),
            Self::Body(_) => None,
        }
    }

    /// Returns the crate that contains the object identified by this ref, regardless of whether
    /// the object originates in a crate or in a body.
    ///
    /// This method must not be confused with `as_crate_ref`.
    pub fn origin_crate(&self) -> CrateRef {
        match self {
            Self::Crate(crate_ref) => *crate_ref,
            Self::Body(body) => body.crate_ref,
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

impl ModuleRef {
    pub fn krate(crate_ref: CrateRef, module: ModuleId) -> Self {
        Self {
            origin: DefMapRef::Crate(crate_ref),
            module,
        }
    }
}

/// Stable reference to one local definition across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LocalDefRef {
    pub origin: DefMapRef,
    pub local_def: LocalDefId,
}

/// Stable reference to one enum variant across the whole project analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LocalEnumVariantRef {
    pub origin: DefMapRef,
    pub local_enum_variant: LocalEnumVariantId,
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

/// Namespace-resolved crate-level definition reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum DefId {
    Module(ModuleRef),
    Local(LocalDefRef),
    EnumVariant(LocalEnumVariantRef),
}
