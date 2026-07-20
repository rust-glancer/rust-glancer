//! Body IR materialization choices for fresh builds and package rebuilds.
//!
//! The project layer decides when indexing may start early or when a query needs more analysis
//! data. Once that decision reaches this crate, the question is narrower: which Body IR payloads
//! should this rebuild actually lower, and what crate coverage should the store report afterward?

use rg_def_map::PackageSlot;
use rg_parse::FileId;

use crate::{BodyIrBuildPolicy, BodyIrFile, CrateBodiesCoverage};

/// Owned Body IR materialization plan stored by a package rebuilder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyIrMaterializationPlan {
    /// Lower the full body surface selected by the configured package policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Preserve crate coverage records, but leave body-bearing crates unlowered for later.
    CoverageOnly(BodyIrBuildPolicy),
    /// Lower bodies from selected files while preserving missing/partial crate coverage.
    SelectedFiles(Vec<BodyIrFile>),
}

impl BodyIrMaterializationPlan {
    pub(super) fn lowering(&self) -> BodyIrMaterialization<'_> {
        match self {
            Self::ConfiguredBodies(policy) => BodyIrMaterialization::ConfiguredBodies(*policy),
            Self::CoverageOnly(policy) => BodyIrMaterialization::CoverageOnly(*policy),
            Self::SelectedFiles(files) => BodyIrMaterialization::SelectedFiles(files),
        }
    }
}

/// Borrowed materialization mode used while lowering crate bodies.
#[derive(Debug, Clone, Copy)]
pub(super) enum BodyIrMaterialization<'a> {
    /// Lower every body selected by the package policy.
    ConfiguredBodies(BodyIrBuildPolicy),
    /// Create the same crate slots as a policy build, but mark body-bearing crates as missing.
    CoverageOnly(BodyIrBuildPolicy),
    /// Lower only selected source files. Crates can become partial when other body files remain.
    SelectedFiles(&'a [BodyIrFile]),
}

impl BodyIrMaterialization<'_> {
    pub(super) fn crate_coverage(
        self,
        package: PackageSlot,
        parse_package: &rg_parse::Package,
        files_with_bodies: &[FileId],
    ) -> CrateBodiesCoverage {
        match self {
            Self::ConfiguredBodies(policy) => {
                if policy.should_lower_package(parse_package) {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::SkippedByPolicy
                }
            }
            Self::CoverageOnly(policy) => {
                if !policy.should_lower_package(parse_package) {
                    return CrateBodiesCoverage::SkippedByPolicy;
                }

                if files_with_bodies.is_empty() {
                    CrateBodiesCoverage::Complete
                } else {
                    CrateBodiesCoverage::Missing
                }
            }
            Self::SelectedFiles(files) => {
                let package_selected = files.iter().any(|file| file.package == package);
                if !package_selected {
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
                        .any(|file| file.package == package && file.file == *file_id);
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
        }
    }

    pub(super) fn should_lower_body_file(self, package: PackageSlot, file_id: FileId) -> bool {
        match self {
            Self::ConfiguredBodies(_) => true,
            Self::CoverageOnly(_) => false,
            Self::SelectedFiles(files) => files
                .iter()
                .any(|file| file.package == package && file.file == file_id),
        }
    }
}
