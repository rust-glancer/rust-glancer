//! Body IR materialization choices for fresh builds and package rebuilds.
//!
//! The project layer decides when indexing may start early or when a query needs more analysis
//! data. Once that decision reaches this crate, the question is narrower: which Body IR payloads
//! should this rebuild actually lower, and what crate coverage should the store report afterward?

use rg_ir_model::CrateRef;
use rg_parse::FileId;
use rg_std::UniqueVec;

use crate::{BodyIrBuildPolicy, BodyIrFile, CrateBodiesCoverage};

/// Owned Body IR materialization plan stored by a package builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyIrMaterializationPlan {
    /// Lower the full body surface selected by the configured build policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Preserve crate coverage records, but leave body-bearing crates unlowered for later.
    CoverageOnly(BodyIrBuildPolicy),
    /// Lower bodies from selected files while preserving missing/partial crate coverage.
    SelectedFiles(Vec<BodyIrFile>),
    /// Lower every body in selected semantic crates while leaving sibling targets untouched.
    SelectedCrates(UniqueVec<CrateRef>),
}

impl BodyIrMaterializationPlan {
    pub(super) fn lowering(&self) -> BodyIrMaterialization<'_> {
        match self {
            Self::ConfiguredBodies(policy) => BodyIrMaterialization::ConfiguredBodies(*policy),
            Self::CoverageOnly(policy) => BodyIrMaterialization::CoverageOnly(*policy),
            Self::SelectedFiles(files) => BodyIrMaterialization::SelectedFiles(files),
            Self::SelectedCrates(crates) => BodyIrMaterialization::SelectedCrates(crates),
        }
    }
}

/// Borrowed materialization mode used while lowering crate bodies.
#[derive(Debug, Clone, Copy)]
pub(super) enum BodyIrMaterialization<'a> {
    /// Lower every body selected by the build policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Create the same crate slots as a policy build, but mark body-bearing crates as missing.
    CoverageOnly(BodyIrBuildPolicy),
    /// Lower only selected source files. Crates can become partial when other body files remain.
    SelectedFiles(&'a [BodyIrFile]),
    /// Lower every body belonging to the selected semantic crates.
    SelectedCrates(&'a UniqueVec<CrateRef>),
}

impl BodyIrMaterialization<'_> {
    /// Decide what coverage the crate slot reports after this materialization pass.
    ///
    /// Coverage describes work remaining for the complete semantic crate, not merely whether this
    /// pass selected something. Selecting one of two body-bearing files is therefore `Partial`,
    /// while selecting the whole crate or a selected crate with no bodies is `Complete`.
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
            Self::SelectedFiles(files) => {
                let crate_selected = files.iter().any(|file| file.crate_ref == crate_ref);
                if !crate_selected {
                    return CrateBodiesCoverage::Missing;
                }

                if files_with_bodies.is_empty() {
                    return CrateBodiesCoverage::Complete;
                }

                let mut selected_body_file_seen = false;
                let mut unselected_body_file_seen = false;
                for file_id in files_with_bodies {
                    let selected = files
                        .iter()
                        .any(|file| file.crate_ref == crate_ref && file.file == *file_id);
                    if selected {
                        selected_body_file_seen = true;
                    } else {
                        unselected_body_file_seen = true;
                    }
                }

                match (selected_body_file_seen, unselected_body_file_seen) {
                    (true, false) => CrateBodiesCoverage::Complete,
                    (true, true) => CrateBodiesCoverage::Partial,
                    (false, true) => CrateBodiesCoverage::Missing,
                    (false, false) => CrateBodiesCoverage::Complete,
                }
            }
            Self::SelectedCrates(crates) => {
                if crates.contains(&crate_ref) || files_with_bodies.is_empty() {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::Missing
                }
            }
        }
    }

    pub(super) fn should_lower_body_file(self, crate_ref: CrateRef, file_id: FileId) -> bool {
        match self {
            Self::ConfiguredBodies(_) => true,
            Self::CoverageOnly(_) => false,
            Self::SelectedFiles(files) => files
                .iter()
                .any(|file| file.crate_ref == crate_ref && file.file == file_id),
            Self::SelectedCrates(crates) => crates.contains(&crate_ref),
        }
    }

    /// Returns whether an exact-target rebuild should replace this crate's prior payload.
    pub(super) fn selects_crate(self, crate_ref: CrateRef) -> bool {
        match self {
            Self::ConfiguredBodies(_) | Self::CoverageOnly(_) => true,
            Self::SelectedFiles(files) => files.iter().any(|file| file.crate_ref == crate_ref),
            Self::SelectedCrates(crates) => crates.contains(&crate_ref),
        }
    }
}
