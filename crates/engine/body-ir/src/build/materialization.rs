//! Chooses which saved-source bodies to analyze and which to leave for later.
//!
//! The project layer decides when indexing may start early or when a query needs more analysis
//! data. Once that decision reaches this crate, the question is narrower: which crates and files
//! should this build lower, and what coverage should each resulting crate report?

use rg_ir_model::{CrateRef, FileId};
use rg_std::UniqueVec;

use crate::{BodyIrBuildPolicy, BodyIrFile, CrateBodiesCoverage};

/// Owned selection kept by the Body IR builder until lowering begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyIrMaterializationPlan {
    /// Select every body allowed by the configured build policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Lower selected files and every body in selected crates, omitting unrelated targets.
    /// A whole-crate selection includes its files, so listing a file from that crate does not
    /// create a second build or restrict the crate's coverage.
    Selected {
        files: Vec<BodyIrFile>,
        crates: UniqueVec<CrateRef>,
    },
}

impl BodyIrMaterializationPlan {
    pub(super) fn lowering(&self) -> BodyIrMaterialization<'_> {
        match self {
            Self::ConfiguredBodies(policy) => BodyIrMaterialization::ConfiguredBodies(*policy),
            Self::Selected { files, crates } => BodyIrMaterialization::Selected { files, crates },
        }
    }
}

/// Borrowed materialization mode used while lowering crate bodies.
#[derive(Debug, Clone, Copy)]
pub(super) enum BodyIrMaterialization<'a> {
    /// Lower every body selected by the build policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Create empty entries that record which targets still need body analysis. This lets
    /// declaration queries run while body analysis is postponed.
    CoverageOnly(BodyIrBuildPolicy),
    /// Lower selected files together with complete selected crates. Whole-crate selections take
    /// precedence; file-only selections record partial coverage when other body files remain.
    Selected {
        files: &'a [BodyIrFile],
        crates: &'a UniqueVec<CrateRef>,
    },
}

impl BodyIrMaterialization<'_> {
    /// Describe which files this build will have analyzed for the crate.
    ///
    /// If `lib.rs` and `model.rs` both contain bodies, selecting only `lib.rs` records its file id
    /// in `Files`. Selecting all body-bearing files, or the whole crate, records `Complete`.
    /// File coverage also keeps selected empty files, so queries do not repeatedly request them.
    pub(super) fn crate_coverage(
        self,
        crate_ref: CrateRef,
        parse_package: &rg_parse::Package,
        parse_target: &rg_parse::CargoTarget,
        files_with_bodies: &[FileId],
    ) -> CrateBodiesCoverage {
        match self {
            Self::ConfiguredBodies(policy) => {
                if policy.should_lower_target(parse_package, parse_target) {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::SkippedByPolicy
                }
            }
            Self::CoverageOnly(policy) => {
                if !policy.should_lower_target(parse_package, parse_target) {
                    return CrateBodiesCoverage::SkippedByPolicy;
                }

                if files_with_bodies.is_empty() {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::Missing
                }
            }
            Self::Selected { files, crates } => {
                if crates.contains(&crate_ref) {
                    return CrateBodiesCoverage::Complete;
                }
                let crate_selected = files.iter().any(|file| file.crate_ref == crate_ref);
                if !crate_selected {
                    return CrateBodiesCoverage::Missing;
                }

                if files_with_bodies.is_empty() {
                    return CrateBodiesCoverage::Complete;
                }

                let selected = files
                    .iter()
                    .filter(|file| file.crate_ref == crate_ref)
                    .map(|file| file.file)
                    .collect::<Vec<_>>();
                if files_with_bodies.iter().all(|file| selected.contains(file)) {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::files(selected)
                }
            }
        }
    }

    pub(super) fn should_lower_body_file(self, crate_ref: CrateRef, file_id: FileId) -> bool {
        match self {
            Self::ConfiguredBodies(_) => true,
            Self::CoverageOnly(_) => false,
            Self::Selected { files, crates } => {
                crates.contains(&crate_ref)
                    || files
                        .iter()
                        .any(|file| file.crate_ref == crate_ref && file.file == file_id)
            }
        }
    }

    /// Include this crate in the build output. Exact selections omit unrelated targets.
    pub(super) fn selects_crate(self, crate_ref: CrateRef) -> bool {
        match self {
            Self::ConfiguredBodies(_) | Self::CoverageOnly(_) => true,
            Self::Selected { files, crates } => {
                crates.contains(&crate_ref) || files.iter().any(|file| file.crate_ref == crate_ref)
            }
        }
    }
}
