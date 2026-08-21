//! Retained Body IR materialization coverage for crates and packages.

use rg_ir_model::CrateId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Compact crate coverage retained for one package even when its body payload is offloaded.
///
/// Queries only need this directory to decide whether a target must be materialized. An offloaded
/// package keeps it directly in its package-store entry instead of reopening the cache artifact or
/// retaining the much larger body arenas.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub struct PackageBodiesCoverage {
    crates: Box<[CrateBodiesCoverage]>,
}

impl PackageBodiesCoverage {
    /// Preserve the semantic-crate order used by `CrateId` inside this package.
    pub fn from_crates(crates: Vec<CrateBodiesCoverage>) -> Self {
        Self {
            crates: crates.into_boxed_slice(),
        }
    }

    pub(crate) fn crate_coverage(&self, crate_id: CrateId) -> Option<CrateBodiesCoverage> {
        self.crates.get(crate_id.0).copied()
    }
}

/// How much of a crate's body surface has been materialized.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum CrateBodiesCoverage {
    /// Every semantic item body known for the crate was considered for lowering.
    #[display("complete")]
    Complete,
    /// At least one, but not every, known body source file was selected for lowering.
    #[display("partial")]
    Partial,
    /// The crate has body sources, but none of them were selected for this materialization pass.
    #[display("missing")]
    Missing,
    /// The configured package-and-target policy intentionally did not build bodies for this crate.
    #[display("skipped-by-policy")]
    SkippedByPolicy,
}

impl CrateBodiesCoverage {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub fn is_materialized(self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }

    pub fn status(self) -> CrateBodiesStatus {
        match self {
            Self::Complete | Self::Partial => CrateBodiesStatus::Built,
            Self::Missing | Self::SkippedByPolicy => CrateBodiesStatus::Skipped,
        }
    }
}

/// Whether one crate's bodies were eagerly lowered.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum CrateBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}
