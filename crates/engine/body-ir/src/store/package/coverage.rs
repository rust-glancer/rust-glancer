//! Records which crates and source files already have body analysis.

use rg_ir_model::{CrateId, FileId};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Stores [`CrateBodiesCoverage`] for each target in a package, in [`CrateId`] order.
///
/// The project keeps this small list in memory when the full [`PackageBodies`](crate::PackageBodies)
/// is stored on disk. Queries use it to decide whether they need to analyze more bodies or can
/// load results that are already in the cache.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub struct PackageBodiesCoverage {
    crates: Box<[CrateBodiesCoverage]>,
}

impl PackageBodiesCoverage {
    /// Keep entries in the order used by [`CrateId`] inside this package.
    pub fn from_crates(crates: Vec<CrateBodiesCoverage>) -> Self {
        Self {
            crates: crates.into_boxed_slice(),
        }
    }

    pub(crate) fn crate_coverage(&self, crate_id: CrateId) -> Option<&CrateBodiesCoverage> {
        self.crates.get(crate_id.0)
    }
}

/// Records which source files have body analysis for one crate, including files with no bodies.
///
/// Queries use this to decide whether more analysis is needed before reading a file. The record
/// belongs to one Cargo target: analyzing a shared file in a library does not make that file's
/// bodies ready in an integration-test target too.
#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub enum CrateBodiesCoverage {
    /// All files containing bodies in this crate have been analyzed.
    #[display("complete")]
    Complete,
    /// Files already analyzed, sorted and unique. Includes files with no bodies so queries do not
    /// request them again.
    #[display("partial")]
    Files(Box<[FileId]>),
    /// Bodies have not been analyzed yet for this target.
    #[display("missing")]
    Missing,
    /// The configured build policy leaves this target for an explicit query to request.
    #[display("skipped-by-policy")]
    SkippedByPolicy,
}

impl CrateBodiesCoverage {
    pub fn files(mut files: Vec<FileId>) -> Self {
        files.sort_unstable_by_key(|file| file.0);
        files.dedup();
        Self::Files(files.into_boxed_slice())
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }

    pub fn is_materialized(&self) -> bool {
        matches!(self, Self::Complete | Self::Files(_))
    }

    pub fn contains_file(&self, file: FileId) -> bool {
        match self {
            Self::Complete => true,
            Self::Files(files) => files.binary_search_by_key(&file.0, |file| file.0).is_ok(),
            Self::Missing | Self::SkippedByPolicy => false,
        }
    }

    /// Whether `self` already covers every file analyzed by `other`.
    ///
    /// For example, coverage of `lib.rs` and `model.rs` contains coverage of just `lib.rs`.
    /// Missing and policy-skipped coverage carry no processed work, so every result contains them.
    pub fn contains(&self, other: &Self) -> bool {
        match other {
            Self::Missing | Self::SkippedByPolicy => true,
            Self::Complete => self.is_complete(),
            Self::Files(files) => {
                self.is_materialized() && files.iter().all(|&file| self.contains_file(file))
            }
        }
    }

    pub fn status(&self) -> CrateBodiesStatus {
        if self.is_materialized() {
            CrateBodiesStatus::Built
        } else {
            CrateBodiesStatus::Skipped
        }
    }
}

/// Whether body analysis is available for part or all of a crate.
/// [`CrateBodiesCoverage`] records how much.
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
