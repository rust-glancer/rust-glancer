//! Builds missing body analysis and adds the results to the saved project.
//!
//! A query can ask for the files or crates it needs through [`SplitIndexing::materialize`]. Work
//! that runs elsewhere first captures [`SavedBodyBuildInputs`], then returns [`SavedBodyProducts`]
//! for [`SplitIndexing::publish`]. Publication checks that the saved source has not been replaced
//! and preserves any body analysis completed by other requests in the meantime.

mod build;
mod publication;

use super::{Project, state::ProjectState};
use crate::{
    ProjectMemoryPurgePoint,
    profile::{BuildMemorySampler, BuildProcessMemory, record_build_checkpoint},
};
use anyhow::Context as _;
use rg_body_ir::{BodyIrBuildProgress, BodyIrBuildStage, CrateBodiesCoverage, PackageBodies};
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateId, CrateRef};
use rg_parse::FileId;

pub use self::build::{SavedBodyBuildInputs, SavedBodyProducts};
pub use self::publication::{BodyPublication, BodyPublicationOutcome};

/// Files and crates whose body analysis a query needs before it can run.
///
/// A file is paired with a [`CrateRef`] because the same source may be compiled by several Cargo
/// targets with different imports or cfg options. A hover usually needs one such file; reference
/// search may need both selected files and entire crates. Pass the selection to
/// [`SplitIndexing::materialize`] or [`SplitIndexing::prepare`].
#[derive(Debug, Clone, Copy)]
pub enum AnalysisSurface<'a> {
    /// Request body analysis for these files in their specified crates.
    Files(&'a [(CrateRef, FileId)]),
    /// Request body analysis for every file in these crates.
    Crates(&'a [CrateRef]),
    /// Prepare both selections in one build. A whole-crate request includes any listed files
    /// from that crate, so those files do not need separate products.
    FilesAndCrates {
        files: &'a [(CrateRef, FileId)],
        crates: &'a [CrateRef],
    },
}

/// Stage reported while analyzing bodies through [`SavedBodyBuildInputs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitIndexingStage {
    LoweringBodies,
    ResolvingBodies,
}

/// Completed package count within one [`SplitIndexingStage`].
///
/// Package completion is deliberately reported instead of an elapsed-time percentage. Packages
/// vary widely in size, but the count still tells callers that work is advancing and gives every
/// stage an exact terminal value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitIndexingProgress {
    stage: SplitIndexingStage,
    completed_packages: usize,
    total_packages: usize,
}

impl SplitIndexingProgress {
    fn from_body_ir(progress: BodyIrBuildProgress) -> Self {
        let stage = match progress.stage() {
            BodyIrBuildStage::Lowering => SplitIndexingStage::LoweringBodies,
            BodyIrBuildStage::Resolving => SplitIndexingStage::ResolvingBodies,
        };
        Self {
            stage,
            completed_packages: progress.completed_packages(),
            total_packages: progress.total_packages(),
        }
    }

    pub fn stage(self) -> SplitIndexingStage {
        self.stage
    }

    pub fn completed_packages(self) -> usize {
        self.completed_packages
    }

    pub fn total_packages(self) -> usize {
        self.total_packages
    }
}

/// Builds missing body analysis and adds it to the saved [`Project`].
///
/// [`Self::materialize`] prepares the files or crates needed by a query. To build on another
/// thread, use [`Self::prepare`], build the returned inputs, then call [`Self::publish`].
pub struct SplitIndexing<'project> {
    project: &'project mut Project,
}

impl<'project> SplitIndexing<'project> {
    pub(super) fn new(project: &'project mut Project) -> Self {
        Self { project }
    }

    /// Finish missing or partial body analysis and install it in the project.
    ///
    /// Targets still marked [`CrateBodiesCoverage::SkippedByPolicy`] stay skipped. Queries can
    /// start their analysis through [`Self::materialize`] or [`Self::prepare`]; any partial
    /// results from those queries are included in the work to finish.
    pub fn finish(&mut self) -> anyhow::Result<()> {
        self.finish_with_sampler(&mut BuildMemorySampler::disabled())
    }

    pub fn finish_profiled(
        &mut self,
        sampler: impl FnMut() -> Option<BuildProcessMemory> + 'static,
    ) -> anyhow::Result<()> {
        self.finish_with_sampler(&mut BuildMemorySampler::retained(Some(Box::new(sampler))))
    }

    fn finish_with_sampler(&mut self, sampler: &mut BuildMemorySampler) -> anyhow::Result<()> {
        let cancellation = rg_std::CancellationToken::new();
        let inputs = SavedBodyBuildInputs::deferred(&self.project.state);
        let products = inputs
            .build(&cancellation)
            .context("finish deferred body construction")?;
        // Completed products are still private allocations here. Keep them in the active working
        // set without reporting them as retained saved-project payloads before publication.
        let process_memory = sampler.sample_process_memory();
        let project_bytes = sampler.measure_retained(&self.project.state);
        let product_bytes = sampler.measure_retained(&products);
        let active_bytes = sampler.sum_retained(&[project_bytes, product_bytes]);
        record_build_checkpoint(
            "after deferred body construction",
            project_bytes,
            active_bytes,
            process_memory,
        );
        self.publish(products, &cancellation)
            .context("publish deferred body products")?;
        let state = &mut self.project.state;
        state
            .memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterBodyIrBuild);
        let process_memory = sampler.sample_process_memory();
        let project_bytes = sampler.measure_retained(state);
        record_build_checkpoint(
            "after deferred body publication",
            project_bytes,
            project_bytes,
            process_memory,
        );
        if state.compact_if_fully_offloaded() {
            state
                .memory_hooks
                .purge(ProjectMemoryPurgePoint::AfterDeferredIndexingFinish);
            let process_memory = sampler.sample_process_memory();
            let project_bytes = sampler.measure_retained(state);
            record_build_checkpoint(
                "after deferred indexing compaction",
                project_bytes,
                project_bytes,
                process_memory,
            );
        }
        Ok(())
    }

    /// Whether any requested file or crate still needs body analysis.
    ///
    /// Bodies already stored in the package cache count as ready. This checks their recorded
    /// [`CrateBodiesCoverage`] without loading the bodies from disk.
    pub fn needs_materialization(&self, surface: AnalysisSurface<'_>) -> bool {
        let (files, crates) = surface.parts();
        files.iter().any(|&(crate_ref, file)| {
            !self
                .project
                .state
                .body_ir
                .crate_coverage(crate_ref)
                .is_some_and(|coverage| coverage.contains_file(file))
        }) || crates.iter().any(|&crate_ref| {
            !self
                .project
                .state
                .body_ir
                .crate_coverage(crate_ref)
                .is_some_and(CrateBodiesCoverage::is_complete)
        })
    }

    /// Capture the source, declarations and missing body work needed by `surface`.
    ///
    /// The returned [`SavedBodyBuildInputs`] can be built on another thread and submitted through
    /// [`Self::publish`]. File requests include files already analyzed in the same crate, so
    /// replacing its bodies later preserves that work. For a package stored on disk, any target
    /// needing more analysis is rebuilt in full to keep its cache entry complete.
    pub fn prepare(&self, surface: AnalysisSurface<'_>) -> SavedBodyBuildInputs {
        SavedBodyBuildInputs::for_surface(&self.project.state, surface)
    }

    /// Analyze missing bodies for `surface` and install them before the query runs.
    ///
    /// This combines [`Self::prepare`], [`SavedBodyBuildInputs::build`] and [`Self::publish`]
    /// under one mutable project borrow, so no other result can change the selection in between.
    #[rg_std::cancelable("materialize query surface", token = cancellation)]
    pub fn materialize(
        &mut self,
        surface: AnalysisSurface<'_>,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<()> {
        if !self.needs_materialization(surface) {
            return Ok(());
        }
        let inputs = self.prepare(surface);
        #[cfg(test)]
        crate::tests::cancellation::materialization_checkpoint(
            crate::tests::cancellation::MaterializationPoint::InputsPrepared,
            cancellation,
        );
        let products = inputs
            .build(cancellation)
            .context("construct query body products")?;
        let publication = self
            .publish(products, cancellation)
            .context("publish query body products")?;
        // This synchronous path holds the only writer. Background callers can receive ReplanRequired
        // and capture another cumulative selection after their result loses a coverage race.
        debug_assert!(
            publication
                .outcomes()
                .iter()
                .all(|(_, outcome)| *outcome != BodyPublicationOutcome::ReplanRequired)
        );
        Ok(())
    }

    /// Install completed body analysis in the live [`Project`].
    ///
    /// Results from an older saved project version are discarded. For the same version, a crate
    /// is replaced only when the result preserves all files already analyzed there and adds more
    /// coverage. [`BodyPublication`] reports which crates changed or need another call to
    /// [`Self::prepare`].
    ///
    /// Changes commit one package at a time. Cancellation or a cache error can leave earlier
    /// packages installed. Within a package, no cancellation check separates replacing its cache
    /// file from updating the in-memory record of which bodies are available.
    pub fn publish(
        &mut self,
        products: SavedBodyProducts,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<BodyPublication> {
        publication::publish(&mut self.project.state, products, cancellation)
    }
}

impl<'a> AnalysisSurface<'a> {
    fn parts(self) -> (&'a [(CrateRef, FileId)], &'a [CrateRef]) {
        match self {
            Self::Files(files) => (files, &[]),
            Self::Crates(crates) => (&[], crates),
            Self::FilesAndCrates { files, crates } => (files, crates),
        }
    }
}

/// Select targets whose saved coverage still needs work under the configured body policy.
pub(super) fn unfinished_crates(state: &ProjectState) -> impl Iterator<Item = CrateRef> + '_ {
    state
        .parse
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
            !state
                .body_ir
                .crate_coverage(crate_ref)
                .is_some_and(|coverage| configured_crate_is_finished(state, crate_ref, coverage))
        })
}

/// Whether this in-memory package has enough body analysis to be written to the cache.
/// Every target must be complete or skipped by policy; partial file results stay in memory.
pub(crate) fn package_deferred_payload_is_durable(
    state: &ProjectState,
    package: PackageSlot,
) -> bool {
    state
        .body_ir
        .resident_package(package)
        .is_some_and(|bodies| configured_package_is_finished(state, package, bodies))
}

fn configured_package_is_finished(
    state: &ProjectState,
    package: PackageSlot,
    bodies: &PackageBodies,
) -> bool {
    bodies.crates().iter().enumerate().all(|(index, bodies)| {
        configured_crate_is_finished(
            state,
            CrateRef {
                package,
                crate_id: CrateId(index),
            },
            bodies.coverage(),
        )
    })
}

fn configured_crate_is_finished(
    state: &ProjectState,
    crate_ref: CrateRef,
    coverage: &CrateBodiesCoverage,
) -> bool {
    if coverage.is_complete() {
        return true;
    }
    if !matches!(coverage, CrateBodiesCoverage::SkippedByPolicy) {
        return false;
    }
    let Some(package) = state.parse.package(crate_ref.package.0) else {
        return false;
    };
    let Some(target) = package.targets().get(crate_ref.crate_id.0) else {
        return false;
    };
    !state.body_ir_policy.should_lower_target(package, target)
}
