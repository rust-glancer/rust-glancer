use std::{path::Path, sync::Arc};

use rg_analysis::Analysis;
use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::{DefMapDb, PackageSlot, TargetRef};
use rg_package_store::{PackageStoreError, PackageSubset};
use rg_parse::{FileId, ParseDb};
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;
use rg_workspace::{CargoMetadataConfig, WorkspaceMetadata};

use crate::{
    PackageResidencyPlan, PackageResidencyPolicy, ProjectMemoryHooks,
    cache::{Fingerprint, PackageCacheStore, WorkspaceCachePlan},
};

use super::{stats::ProjectStats, txn::ProjectReadTxn};

/// Fully built project pipeline state.
#[derive(Debug, Clone)]
pub(crate) struct ProjectState {
    pub(crate) workspace: WorkspaceMetadata,
    pub(crate) cargo_metadata_config: CargoMetadataConfig,
    pub(crate) cache_plan: WorkspaceCachePlan,
    pub(crate) cache_store: PackageCacheStore,
    pub(crate) package_source_fingerprints: Vec<Option<Fingerprint>>,
    pub(crate) body_ir_policy: BodyIrBuildPolicy,
    pub(crate) package_residency_policy: PackageResidencyPolicy,
    pub(crate) package_residency: PackageResidencyPlan,
    pub(crate) memory_hooks: Arc<dyn ProjectMemoryHooks>,
    pub(crate) names: PackageNameInterners,
    pub(crate) parse: ParseDb,
    pub(crate) def_map: DefMapDb,
    pub(crate) semantic_ir: SemanticIrDb,
    pub(crate) body_ir: BodyIrDb,
}

impl ProjectState {
    /// Returns the normalized workspace metadata this project was built from.
    pub(crate) fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace
    }

    /// Returns package residency decisions for this project snapshot.
    pub(crate) fn package_residency_plan(&self) -> &PackageResidencyPlan {
        &self.package_residency
    }

    /// Returns the parse database built for this project.
    pub(crate) fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    /// Returns coarse status counters without exposing raw phase databases.
    pub(crate) fn stats(&self) -> ProjectStats {
        ProjectStats::capture(self)
    }

    pub(crate) fn parse_db_mut(&mut self) -> &mut ParseDb {
        &mut self.parse
    }

    /// Starts a read transaction over resident and lazy-loadable offloaded packages.
    pub(crate) fn read_txn(&self) -> anyhow::Result<ProjectReadTxn<'_>> {
        ProjectReadTxn::new(self)
    }

    pub(crate) fn read_txn_for_subset(
        &self,
        subset: &PackageSubset,
    ) -> anyhow::Result<ProjectReadTxn<'_>> {
        ProjectReadTxn::for_subset(self, subset)
    }

    /// Returns the high-level query API for this frozen project analysis.
    pub(crate) fn analysis<'a>(&self, txn: &ProjectReadTxn<'a>) -> Analysis<'a> {
        Analysis::new(txn.analysis())
    }

    /// Iterates over non-sysroot package slots from the current Cargo graph.
    ///
    /// Phase payloads may be offloaded, but package slots remain the stable ids that connect
    /// workspace metadata, parse metadata, and user-visible change summaries.
    pub(crate) fn non_sysroot_package_slots(&self) -> impl Iterator<Item = PackageSlot> + '_ {
        self.workspace
            .packages()
            .iter()
            .zip(self.parse.packages())
            .enumerate()
            .filter(|(_, (package, _))| !package.origin.is_sysroot())
            .map(|(package_idx, _)| PackageSlot(package_idx))
    }

    /// Returns all targets declared by the given package slot.
    pub(crate) fn target_refs_for_package(&self, package: PackageSlot) -> Vec<TargetRef> {
        let Some(parsed_package) = self.parse.package(package.0) else {
            return Vec::new();
        };

        parsed_package
            .targets()
            .iter()
            .map(|target| TargetRef {
                package,
                target: target.id,
            })
            .collect()
    }

    /// Returns all parsed files matching a canonical filesystem path.
    pub(crate) fn file_refs_for_path(&self, canonical_path: &Path) -> Vec<ProjectFileRef> {
        let mut files = Vec::new();

        for (package_idx, parsed_package) in self.parse.packages().iter().enumerate() {
            for parsed_file in parsed_package.parsed_files() {
                if parsed_file.path() != canonical_path {
                    continue;
                }

                files.push(ProjectFileRef {
                    package: PackageSlot(package_idx),
                    file: parsed_file.file_id(),
                });
            }
        }

        files
    }

    pub(crate) fn is_recoverable_cache_load_failure(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<PackageStoreError>(),
                Some(PackageStoreError::Load { .. })
            )
        })
    }
}

/// One package-local parsed file in the project graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectFileRef {
    pub(crate) package: PackageSlot,
    pub(crate) file: FileId,
}
