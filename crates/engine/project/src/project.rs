//! Owns one saved analysis project and replaces it only after a successful rebuild.
//!
//! A saved change is first applied to a private candidate. The candidate becomes the new `Project`
//! only after it has been fully built and its source has been checked. Queries may load package
//! data that was left on disk, but loading that data does not create another source generation.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rg_ir_model::PackageSlot;
use rg_std::MemorySize;
use rg_workspace::WorkspaceMetadata;

use crate::{
    IndexingPerformancePreference, PackageBatchSize, PackageResidencyPlan, ProjectBuilder,
    ProjectGenerationId, ProjectSnapshot, ProjectStats, SavedBodyBuildInputs, SplitIndexing,
    indexing, selection::PhasePackageSet, state::ProjectState,
};

/// Mutable owner for the current analysis state.
///
/// `Project` is the host-facing state container: it accepts saved file changes, refreshes the
/// derived phase databases, and hands out immutable snapshots for queries.
#[derive(Debug, Clone, MemorySize)]
pub struct Project {
    pub(crate) state: ProjectState,
}

impl Project {
    /// Starts configuring a fresh analysis project build.
    pub fn builder(workspace: WorkspaceMetadata) -> ProjectBuilder {
        ProjectBuilder::new(workspace)
    }

    /// Returns an immutable query view of the current project state.
    pub fn snapshot(&self) -> ProjectSnapshot<'_> {
        ProjectSnapshot { state: &self.state }
    }

    /// Returns the identity of the successfully published saved-source generation.
    pub fn generation_id(&self) -> ProjectGenerationId {
        self.state.generation_id()
    }

    /// Returns the normalized workspace metadata this project was built from.
    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.state.workspace()
    }

    /// Return package slots whose parsed source inventory contains this path.
    pub fn package_slots_for_path(&self, path: &Path) -> anyhow::Result<Vec<PackageSlot>> {
        PhasePackageSet::from_path(&self.state.parse, path).map(PhasePackageSet::into_vec)
    }

    /// Returns package residency decisions for this project.
    pub fn package_residency_plan(&self) -> &PackageResidencyPlan {
        self.state.package_residency_plan()
    }

    /// Returns the indexing speed/memory trade-off used by this project.
    pub fn indexing_preference(&self) -> IndexingPerformancePreference {
        self.state.indexing_preference
    }

    /// Returns the package batch size configured for lower-memory indexing.
    pub fn package_batch_size(&self) -> PackageBatchSize {
        self.state.package_batch_size
    }

    /// Returns coarse status counters without exposing raw phase databases.
    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    /// Copy retained metadata into fresh allocations while the originals are still alive.
    /// The intent behind this method is to lower the OS memory occupied by the process after
    /// indexing. Dropping the originals afterward may let the allocator release pages fragmented
    /// during indexing. How much this helps depends on the allocator, but we have measured lower
    /// idle memory with `mimalloc`.
    ///
    /// Only do this once all analysis payloads have been offloaded; otherwise return `false`
    /// without copying anything. Call this before going idle, then run the allocator cleanup hook
    /// after the old state has been dropped.
    pub fn reallocate_if_fully_offloaded(&mut self) -> bool {
        self.state.reallocate_if_fully_offloaded()
    }

    /// Iterates bounded macro-expansion-limit diagnostics from resident packages.
    pub fn macro_expansion_limit_reports(
        &self,
    ) -> impl Iterator<Item = &rg_def_map::MacroExpansionLimitReport> {
        self.state.def_map.macro_expansion_limit_reports()
    }

    /// Returns whether an analysis error came from disposable package-cache storage.
    pub fn is_recoverable_cache_load_failure(error: &anyhow::Error) -> bool {
        ProjectState::is_recoverable_cache_load_failure(error)
    }

    /// Returns the saved path whose current disk bytes no longer match a frozen source revision.
    pub fn stale_source_path(error: &anyhow::Error) -> Option<&Path> {
        error.chain().find_map(|cause| {
            cause
                .downcast_ref::<rg_source::SourceError>()
                .and_then(rg_source::SourceError::stale_path)
        })
    }

    /// Finds all known saved sources that no longer match this published generation.
    ///
    /// Incremental rebuild recovery uses this only after one candidate has already reported a
    /// source race. Scanning then turns a settled multi-file edit into one retry without adding a
    /// full source-tree scan to ordinary saves.
    pub fn stale_saved_source_paths(&self) -> Result<Vec<PathBuf>, rg_source::SourceError> {
        self.state.parse_db().source_inventory().stale_saved_paths()
    }

    /// Builds a private candidate and publishes it as a new generation only after success.
    ///
    /// Candidate internals remain unreachable while `build` runs. Assigning the public generation
    /// id immediately before the state swap makes that swap the single publication point: failed
    /// work cannot advance live generation identity or leave partially rebuilt phase databases in
    /// the project.
    pub(crate) fn try_publish_generation<T>(
        &mut self,
        build: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.try_publish_generation_when(build, |_| true)
    }

    /// Builds a candidate but publishes it only when its output describes a real state change.
    pub(crate) fn try_publish_generation_when<T>(
        &mut self,
        build: impl FnOnce(&mut Project) -> anyhow::Result<T>,
        should_publish: impl FnOnce(&T) -> bool,
    ) -> anyhow::Result<T> {
        let mut candidate = self.clone();
        let output = build(&mut candidate)
            .context("while attempting to build project generation candidate")?;
        if !should_publish(&output) {
            return Ok(output);
        }

        candidate.state.generation_id = ProjectGenerationId::fresh();
        self.state = candidate.state;
        Ok(output)
    }

    /// Rebuilds the project from source and rewrites offloadable package cache artifacts.
    pub fn recover_after_cache_load_failure(&mut self) -> anyhow::Result<()> {
        self.try_publish_generation(|candidate| {
            indexing::recover_from_cache_load_failure(&mut candidate.state).context(
                "while attempting to recover analysis project after package cache load failed",
            )
        })
    }

    /// Get a [`SplitIndexing`] handle to build missing body analysis and install its results.
    pub fn split_indexing(&mut self) -> SplitIndexing<'_> {
        SplitIndexing::new(self)
    }

    /// Whether any crate still has missing or partial body analysis to finish.
    ///
    /// This determines whether [`Self::deferred_body_build`] has background work to do. Targets
    /// left untouched by the build policy do not count. Once a query has partially analyzed a
    /// skipped target, its remaining files count as unfinished too.
    pub fn has_unfinished_split_indexing(&self) -> bool {
        self.state.unfinished_crates().next().is_some()
    }

    /// Capture inputs for the missing or partial body analysis reported by
    /// [`Self::has_unfinished_split_indexing`].
    ///
    /// Move the returned [`SavedBodyBuildInputs`] to a worker and build them there. Submit its
    /// results through [`SplitIndexing::publish`], which checks that this saved project version
    /// is still current before installing them.
    pub fn deferred_body_build(&self) -> SavedBodyBuildInputs {
        SavedBodyBuildInputs::deferred(&self.state)
    }
}
