//! Whether a target has all, some, or none of its bodies materialized.

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// How much of a target's body surface has been materialized.
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
pub enum TargetBodiesCoverage {
    /// Every semantic item body known for the target was considered for lowering.
    #[display("complete")]
    Complete,
    /// At least one, but not every, known body source file was selected for lowering.
    #[display("partial")]
    Partial,
    /// The target has body sources, but none of them were selected for this materialization pass.
    #[display("missing")]
    Missing,
    /// The configured package policy intentionally did not build bodies for this target.
    #[display("skipped-by-policy")]
    SkippedByPolicy,
}

impl TargetBodiesCoverage {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub fn is_materialized(self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }

    pub fn status(self) -> TargetBodiesStatus {
        match self {
            Self::Complete | Self::Partial => TargetBodiesStatus::Built,
            Self::Missing | Self::SkippedByPolicy => TargetBodiesStatus::Skipped,
        }
    }
}

/// Whether one target's bodies were eagerly lowered.
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
pub enum TargetBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}
