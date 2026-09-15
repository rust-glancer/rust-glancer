//! Phase storage behind one saved `Project` generation.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb, CrateBodiesCoverage, PackageBodies};
use rg_def_map::DefMapDb;
use rg_ir_model::{CrateId, CrateRef, FileId, PackageSlot};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_std::MemorySize;
use rg_text::PackageNameInterners;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use crate::{
    IndexingPerformancePreference, PackageBatchSize, PackageResidencyPlan, PackageResidencyPolicy,
    ProjectMemoryHooks, SplitIndexingMode,
    stats::{MacroExpansionLimitBuildSummary, ProjectStats},
    storage::cache::{Fingerprint, PackageCacheInstance, PackageCacheStore, WorkspaceCachePlan},
};

/// Identifies one saved version of a [`Project`](crate::Project).
///
/// Publishing a source or workspace change gives the project a new id. Adding body analysis for
/// the same source keeps the id, so [`SavedBodyProducts`](crate::SavedBodyProducts) can use it to
/// check that their source and declarations still match before being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
#[memsize(leaf)]
pub struct ProjectGenerationId(u64);

impl ProjectGenerationId {
    pub(crate) fn fresh() -> Self {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Owns the saved source and analysis for one version of [`Project`](crate::Project).
///
/// Package slots are the coherence key across resident and offloaded phases. Parse metadata stays
/// resident for every package so source locations remain addressable, while DefMap, Semantic IR,
/// and Body IR may store a package either resident in memory or offloaded behind the same cache
/// artifact.
#[derive(Debug, Clone, MemorySize)]
pub(crate) struct ProjectState {
    pub(crate) generation_id: ProjectGenerationId,
    pub(crate) workspace: WorkspaceMetadata,
    #[memsize(skip)]
    pub(crate) workspace_lowering_config: WorkspaceLoweringConfig,
    pub(crate) cargo_metadata_config: CargoMetadataConfig,
    pub(crate) cache_plan: WorkspaceCachePlan,
    #[memsize(skip)]
    pub(crate) cache_instance: PackageCacheInstance,
    #[memsize(skip)]
    pub(crate) cache_store: PackageCacheStore,
    pub(crate) package_source_fingerprints: Vec<Option<Fingerprint>>,
    pub(crate) body_ir_policy: BodyIrBuildPolicy,
    #[memsize(skip)]
    pub(crate) split_indexing_mode: SplitIndexingMode,
    #[memsize(skip)]
    pub(crate) indexing_preference: IndexingPerformancePreference,
    #[memsize(skip)]
    pub(crate) package_batch_size: PackageBatchSize,
    pub(crate) package_residency_policy: PackageResidencyPolicy,
    pub(crate) package_residency: PackageResidencyPlan,
    #[memsize(skip)]
    pub(crate) memory_hooks: Arc<dyn ProjectMemoryHooks>,
    pub(crate) names: PackageNameInterners,
    /// Shared with saved body builds. Source updates copy the metadata before changing its file
    /// and target inventory, so outstanding builds keep the source context they started with.
    pub(crate) parse: Arc<ParseDb>,
    pub(crate) macro_expansion_limit_summary: MacroExpansionLimitBuildSummary,
    pub(crate) def_map: DefMapDb,
    pub(crate) semantic_ir: SemanticIrDb,
    pub(crate) body_ir: BodyIrDb,
}

impl ProjectState {
    pub(crate) fn generation_id(&self) -> ProjectGenerationId {
        self.generation_id
    }

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
        Arc::make_mut(&mut self.parse)
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

    /// Returns all semantic crates declared by the given package slot.
    pub(crate) fn crate_refs_for_package(&self, package: PackageSlot) -> Vec<CrateRef> {
        let Some(parsed_package) = self.parse.package(package.0) else {
            return Vec::new();
        };

        parsed_package
            .targets()
            .iter()
            .enumerate()
            .map(|(crate_idx, _)| CrateRef {
                package,
                crate_id: CrateId(crate_idx),
            })
            .collect()
    }

    /// Returns all parsed files matching a canonical filesystem path.
    pub(crate) fn file_refs_for_path(&self, canonical_path: &Path) -> Vec<ProjectFileRef> {
        self.parse
            .file_refs_for_path(canonical_path)
            .into_iter()
            .map(|file| ProjectFileRef {
                package: PackageSlot(file.package),
                file: file.file,
            })
            .collect()
    }

    /// Select targets whose saved coverage still needs work under the configured body policy.
    pub(crate) fn unfinished_crates(&self) -> impl Iterator<Item = CrateRef> + '_ {
        self.parse
            .packages()
            .iter()
            .enumerate()
            .flat_map(|(package, data)| {
                data.targets()
                    .iter()
                    .enumerate()
                    .map(move |(crate_id, _)| CrateRef {
                        package: PackageSlot(package),
                        crate_id: CrateId(crate_id),
                    })
            })
            .filter(move |&crate_ref| {
                !self
                    .body_ir
                    .crate_coverage(crate_ref)
                    .is_some_and(|coverage| self.configured_crate_is_finished(crate_ref, coverage))
            })
    }

    /// Whether this in-memory package has enough body analysis to be written to the cache.
    /// Every target must be complete or skipped by policy; partial file results stay in memory.
    pub(crate) fn package_deferred_payload_is_durable(&self, package: PackageSlot) -> bool {
        self.body_ir
            .resident_package(package)
            .is_some_and(|bodies| self.configured_package_is_finished(package, bodies))
    }

    pub(crate) fn configured_package_is_finished(
        &self,
        package: PackageSlot,
        bodies: &PackageBodies,
    ) -> bool {
        bodies.crates().iter().enumerate().all(|(index, bodies)| {
            self.configured_crate_is_finished(
                CrateRef {
                    package,
                    crate_id: CrateId(index),
                },
                bodies.coverage(),
            )
        })
    }

    fn configured_crate_is_finished(
        &self,
        crate_ref: CrateRef,
        coverage: &CrateBodiesCoverage,
    ) -> bool {
        if coverage.is_complete() {
            return true;
        }
        if !matches!(coverage, CrateBodiesCoverage::SkippedByPolicy) {
            return false;
        }
        let Some(package) = self.parse.package(crate_ref.package.0) else {
            return false;
        };
        let Some(target) = package.targets().get(crate_ref.crate_id.0) else {
            return false;
        };
        !self.body_ir_policy.should_lower_target(package, target)
    }
}

/// One package-local parsed file in the project graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectFileRef {
    pub(crate) package: PackageSlot,
    pub(crate) file: FileId,
}
