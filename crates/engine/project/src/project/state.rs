//! Phase storage behind one saved `Project` generation.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rg_analysis::{Analysis, SavedSourceView};
use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::{DefMapDb, DefMapReadTxn, PackageSlot};
use rg_ir_model::{CrateId, CrateRef};
use rg_package_store::{PackageStoreError, PackageSubset};
use rg_parse::{FileId, ParseDb};
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use crate::{
    IndexingPerformancePreference, PackageBatchSize, PackageResidencyPlan, PackageResidencyPolicy,
    ProjectMemoryHooks,
    cache::{Fingerprint, PackageCacheInstance, PackageCacheStore, WorkspaceCachePlan},
};
use rg_std::MemorySize;

use super::{
    build::SplitIndexingMode,
    loading::PackageReadLoaders,
    stats::{MacroExpansionLimitBuildSummary, ProjectStats},
    txn::ProjectReadTxn,
};

/// Identity of one successfully published saved-source project generation.
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

/// Fully built project generation.
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
    pub(crate) parse: ParseDb,
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

    /// Create one request-owned loader set for this saved project snapshot.
    pub(crate) fn query_read_loaders(&self) -> PackageReadLoaders {
        PackageReadLoaders::new(self)
    }

    /// Starts a def-map-only read transaction over selected package slots.
    pub(crate) fn def_map_read_txn_for_subset(&self, subset: &PackageSubset) -> DefMapReadTxn<'_> {
        let loaders = self.query_read_loaders();
        self.def_map.read_txn_for_subset(loaders.def_map, subset)
    }

    /// Returns the high-level query API for this frozen project analysis.
    pub(crate) fn analysis<'a>(&'a self, txn: &ProjectReadTxn<'a>) -> Analysis<'a> {
        Analysis::new(txn.view_db().clone(), SavedSourceView::new(self.parse_db()))
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
