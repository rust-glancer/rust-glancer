//! Whether a crate has all, some, or none of its bodies materialized.

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

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
    /// The configured package policy intentionally did not build bodies for this crate.
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
