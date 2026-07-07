//! Split indexing policy for a live project.
//!
//! Fresh indexing can return after the structural phases are ready, before every expensive
//! analysis payload has been fully materialized. This module owns the policy after that early-start
//! point: decide what a query must materialize, finish the deferred work in the background, and
//! make sure residency only writes durable package artifacts.
//!
//! There are three paths through the public `SplitIndexing` entrypoint:
//!
//! - `finish` finishes the configured deferred payloads for resident packages and then applies
//!   normal package residency.
//! - `materialize` makes the analysis surface needed by one query available, such as one file for
//!   hover or a mix of files and targets for reference search.
//! - `finish_detached` and `merge_finished` let an LSP background clone finish deferred payloads
//!   and merge package-wise improvements back into the saved project.
//!
//! That last path has to be monotonic. Query-time materialization may finish a package before the
//! background clone finishes, so a stale background result must not replace better saved coverage
//! with an older partial view.

use anyhow::Context as _;
use rg_body_ir::{BodyIrBuildPolicy, BodyIrFile, PackageBodies, TargetBodiesCoverage};
use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_parse::FileId;
use rg_std::{MemorySize, Shrink, UniqueVec};

use crate::{
    ProjectMemoryPurgePoint,
    profile::{BuildMemorySampler, BuildProcessMemory, record_build_checkpoint},
    project::{
        Project, loading::PackageReadLoaders, offloading::ResidencyApplication,
        package_set::PhasePackageSet, state::ProjectState,
    },
};

/// Source surface whose deferred analysis data must be available before a query runs.
///
/// File surfaces are used when a query is tied to a concrete source file. Target surfaces are
/// broader: preparing a target means that all deferred data in its owning package may be needed.
/// Reference search can need both shapes when a text prefilter narrows some work to files while
/// another part of the query still needs whole-target coverage.
#[derive(Debug, Clone, Copy)]
pub enum AnalysisSurface<'a> {
    /// Prepare deferred data needed by analysis rooted in these package-local source files.
    Files(&'a [(PackageSlot, FileId)]),
    /// Prepare all deferred data for the packages that own these targets.
    Targets(&'a [TargetRef]),
    /// Prepare exact file coverage first, then finish target-owning packages.
    FilesAndTargets {
        files: &'a [(PackageSlot, FileId)],
        targets: &'a [TargetRef],
    },
}

/// Split-indexing operations for a live project.
///
/// A project built with `SplitIndexingMode::EarlyStart` is already queryable when this handle is
/// used, but some expensive analysis payloads may still be absent. Callers use this handle to make
/// those payloads real in one of two ways:
///
/// - `materialize` is the on-demand path. It prepares exactly the source surface a query needs
///   before that query runs, so reference search and rename do not miss body-local uses.
/// - `finish` is the background or batch path. It completes the remaining deferred payloads chosen
///   by the project's configured indexing policy and then reapplies package residency.
///
/// Detached finishing uses the same policy, but runs on a cloned `Project`. The clone is wrapped in
/// `FinishedSplitIndexing` until the saved project can merge package-wise improvements without
/// losing any on-demand work that happened meanwhile.
pub struct SplitIndexing<'project> {
    project: &'project mut Project,
}

impl<'project> SplitIndexing<'project> {
    pub(super) fn new(project: &'project mut Project) -> Self {
        Self { project }
    }

    /// Finish deferred indexing without recording build checkpoints.
    pub fn finish(&mut self) -> anyhow::Result<()> {
        finish_unprofiled(&mut self.project.state)
    }

    /// Finish deferred indexing and record retained/process memory checkpoints.
    pub fn finish_profiled(
        &mut self,
        sampler: impl FnMut() -> Option<BuildProcessMemory> + 'static,
    ) -> anyhow::Result<()> {
        let mut memory_sampler = BuildMemorySampler::retained(Some(Box::new(sampler)));
        finish_with_sampler(&mut self.project.state, &mut memory_sampler)
    }

    /// Materialize deferred analysis data for the requested query surface.
    pub fn materialize(&mut self, surface: AnalysisSurface<'_>) -> anyhow::Result<()> {
        materialize_surface(&mut self.project.state, surface)
    }

    /// Merge a detached finish result, returning whether it improved the saved project.
    pub fn merge_finished(&mut self, finished: FinishedSplitIndexing) -> anyhow::Result<bool> {
        merge_finished_project(&mut self.project.state, finished)
    }

    /// Finish deferred indexing inside a detached project clone.
    pub fn finish_detached(project: Project) -> anyhow::Result<FinishedSplitIndexing> {
        finish_detached_project(project)
    }
}

/// Result of finishing split indexing inside a detached project clone.
///
/// The project stays owned by this value until merge time. That lets the saved project decide,
/// package by package, whether the detached result is still an improvement over any on-demand work
/// that happened while the background clone was running.
#[derive(Debug, Clone, MemorySize)]
pub struct FinishedSplitIndexing {
    project: Project,
    packages: Vec<PackageSlot>,
}

/// Finish deferred indexing without recording build checkpoints.
fn finish_unprofiled(state: &mut ProjectState) -> anyhow::Result<()> {
    let mut sampler = BuildMemorySampler::disabled();
    finish_with_sampler(state, &mut sampler)
}

/// Finish deferred indexing and record the memory checkpoints used by profiled builds.
fn finish_with_sampler(
    state: &mut ProjectState,
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<()> {
    let packages = finish_resident_with_sampler(state, sampler)?;
    apply_finished_residency_with_sampler(state, &packages, sampler)
}

/// Make a query-shaped analysis surface available in the saved project before analysis runs.
fn materialize_surface(
    state: &mut ProjectState,
    surface: AnalysisSurface<'_>,
) -> anyhow::Result<()> {
    match surface {
        AnalysisSurface::Files(files) => materialize_files(state, files),
        AnalysisSurface::Targets(targets) => materialize_targets(state, targets),
        AnalysisSurface::FilesAndTargets { files, targets } => {
            materialize_files(state, files)?;
            materialize_targets(state, targets)
        }
    }
}

/// Complete deferred indexing inside a detached project so it can later be merged into saved state.
fn finish_detached_project(mut project: Project) -> anyhow::Result<FinishedSplitIndexing> {
    let mut sampler = BuildMemorySampler::disabled();
    let packages = finish_resident_with_sampler(&mut project.state, &mut sampler)?;
    Ok(FinishedSplitIndexing { project, packages })
}

/// Merge a detached finish result, returning whether it improved the saved project.
fn merge_finished_project(
    state: &mut ProjectState,
    finished: FinishedSplitIndexing,
) -> anyhow::Result<bool> {
    let packages = merge_finished_packages(state, &finished.project.state, &finished.packages)
        .context("while attempting to merge deferred indexing packages")?;
    if packages.is_empty() {
        return Ok(false);
    }

    apply_finished_residency(state, &packages)
        .context("while attempting to apply residency after merging deferred indexing")?;
    Ok(true)
}

/// Returns whether the current deferred-analysis payload can be written to a durable package cache.
///
/// For this split-indexing mode the deferred payload is Body IR. Complete target coverage is
/// durable, and policy-skipped targets are durable when the configured eager policy would skip the
/// whole package anyway. Partial selected-file coverage is intentionally transient: writing it as a
/// package artifact would make future startup-cache reads look complete while silently missing
/// bodies that were never materialized.
pub(crate) fn package_deferred_payload_is_durable(
    state: &ProjectState,
    package: PackageSlot,
) -> bool {
    let Some(parse_package) = state.parse.package(package.0) else {
        return false;
    };
    let Some(body_ir) = state.body_ir.resident_package(package) else {
        return false;
    };

    body_ir.targets().iter().all(|target| {
        target.coverage().is_complete()
            || (!state.body_ir_policy.should_lower_package(parse_package)
                && matches!(target.coverage(), TargetBodiesCoverage::SkippedByPolicy))
    })
}

/// Materialize the deferred payload for a set of source files.
///
/// This is the narrow on-demand path used by file-local LSP queries. The deferred payload is stored
/// as package-shaped Body IR, so the rebuild below keeps any already-materialized files selected as
/// well as the newly requested files.
fn materialize_files(
    state: &mut ProjectState,
    files: &[(PackageSlot, FileId)],
) -> anyhow::Result<()> {
    let mut body_files = files
        .iter()
        .map(|&(package, file)| BodyIrFile::new(package, file))
        .collect::<UniqueVec<_>>();
    if body_files.is_empty() {
        return Ok(());
    }

    // Selected-file rebuilds replace a package payload. Keep already-materialized files selected so
    // preparing one new file cannot accidentally discard earlier on-demand coverage from the same
    // package.
    let requested_packages = PhasePackageSet::from_body_files(body_files.as_slice());
    for package in requested_packages.iter() {
        if let Some(body_ir) = state.body_ir.resident_package(package) {
            extend_with_materialized_body_files(package, body_ir, &mut body_files);
        }
    }

    // Lower only the packages touched by the requested files, but give body lowering the visible
    // dependency subset it needs for type/name facts referenced from those bodies.
    let body_packages = PhasePackageSet::from_body_files(body_files.as_slice());
    let rebuild_subset = body_packages.visible_dependency_subset(&state.workspace);
    let loaders = PackageReadLoaders::new(state);
    let body_ir = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            body_packages.as_slice(),
            &mut state.names,
            loaders.def_map,
            loaders.semantic_ir,
            &rebuild_subset,
        )
        .selected_files(body_files.into_vec())
        .build()
        .context("while attempting to materialize deferred analysis for files")?;

    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);

    // A selected-file request can finish a small package. Once the package is complete, apply the
    // ordinary package-cache/offload rules so restart and idle-memory behavior stay consistent with
    // background finishing.
    let finished_packages = finished_resident_packages(state, body_packages.as_slice());
    apply_finished_residency(state, &finished_packages).context(
        "while attempting to apply residency after preparing deferred analysis for files",
    )?;
    Ok(())
}

/// Treat target readiness as package readiness, because deferred payloads are package-shaped.
fn materialize_targets(state: &mut ProjectState, targets: &[TargetRef]) -> anyhow::Result<()> {
    let packages = PhasePackageSet::from_targets(targets);
    materialize_packages(state, packages.as_slice())
}

/// Complete requested resident packages for a query that cannot safely work from partial data.
fn materialize_packages(state: &mut ProjectState, packages: &[PackageSlot]) -> anyhow::Result<()> {
    let packages = packages_needing_finished_split_indexing(state, packages);
    if packages.is_empty() {
        return Ok(());
    }

    let rebuild_subset =
        PhasePackageSet::from_slice(&packages).visible_dependency_subset(&state.workspace);
    let loaders = PackageReadLoaders::new(state);
    let body_ir = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            &packages,
            &mut state.names,
            loaders.def_map,
            loaders.semantic_ir,
            &rebuild_subset,
        )
        // Query-driven finishing intentionally overrides the eager indexing policy. If a query
        // needs to scan a package, missing bodies would be false negatives rather than saved work.
        .configured_bodies(BodyIrBuildPolicy::all_packages())
        .build()
        .context("while attempting to materialize complete deferred analysis for packages")?;

    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);
    apply_finished_residency(state, &packages).context(
        "while attempting to apply residency after preparing complete deferred analysis for packages",
    )?;
    Ok(())
}

/// Complete the deferred packages selected by the project's configured payload policy.
fn finish_resident_with_sampler(
    state: &mut ProjectState,
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<Vec<PackageSlot>> {
    let packages = unfinished_split_indexing_packages(state);
    if packages.is_empty() {
        return Ok(packages);
    }

    let finish_subset =
        PhasePackageSet::from_slice(&packages).visible_dependency_subset(&state.workspace);
    let loaders = PackageReadLoaders::new(state);
    let body_ir = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            &packages,
            &mut state.names,
            loaders.def_map,
            loaders.semantic_ir,
            &finish_subset,
        )
        .configured_bodies(state.body_ir_policy)
        .build()
        .context("while attempting to finish deferred indexing packages")?;

    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);
    state
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterBodyIrBuild);
    record_project_checkpoint(state, sampler, "after deferred indexing");

    Ok(packages)
}

/// Apply package residency without adding profiler checkpoints.
fn apply_finished_residency(
    state: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    let mut sampler = BuildMemorySampler::disabled();
    apply_finished_residency_with_sampler(state, packages, &mut sampler)
}

/// Turn finished resident packages into their normal cache/offload state.
fn apply_finished_residency_with_sampler(
    state: &mut ProjectState,
    packages: &[PackageSlot],
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    ResidencyApplication::restore(state, packages)
        .apply_profiled(sampler)
        .context("while attempting to apply package cache residency after deferred indexing")?;
    state
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterBodyIrBuild);
    record_project_checkpoint(state, sampler, "after deferred indexing cleanup");

    Ok(())
}

fn record_project_checkpoint(
    state: &ProjectState,
    sampler: &mut BuildMemorySampler,
    label: &'static str,
) {
    let process_memory = sampler.sample_process_memory();
    let project_bytes = sampler.measure_retained(state);
    record_build_checkpoint(label, project_bytes, project_bytes, process_memory);
}

/// Merge only package-wise coverage improvements from a detached background finish.
fn merge_finished_packages(
    state: &mut ProjectState,
    finished: &ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<Vec<PackageSlot>> {
    let mut replacements = Vec::new();

    // Decide every replacement before mutating the saved project. If the finished payload is
    // missing a promised package, fail without leaving a half-merged saved state behind.
    for &package in packages {
        let finished_bodies = finished
            .body_ir
            .resident_package(package)
            .with_context(|| {
                format!(
                    "while attempting to read finished deferred payload package {}",
                    package.0,
                )
            })?;
        let should_replace = state
            .body_ir
            .resident_package(package)
            .map(|current_bodies| {
                body_payload_is_coverage_improvement(current_bodies, finished_bodies)
            })
            .unwrap_or(true);

        // The background clone can lag behind query-time preparation. Keep the saved package when
        // the detached version would only be equal or worse.
        if should_replace {
            replacements.push((package, finished_bodies.clone()));
        }
    }

    if replacements.is_empty() {
        return Ok(Vec::new());
    }

    let mut merged_packages = Vec::with_capacity(replacements.len());
    for (package, bodies) in replacements {
        state
            .body_ir
            .replace_package(package, bodies)
            .with_context(|| {
                format!(
                    "while attempting to merge finished deferred payload package {}",
                    package.0
                )
            })?;
        merged_packages.push(package);
    }
    Shrink::shrink_to_fit(&mut state.names);

    Ok(merged_packages)
}

/// Return resident packages whose configured deferred payload is still incomplete.
fn unfinished_split_indexing_packages(state: &ProjectState) -> Vec<PackageSlot> {
    let mut packages = Vec::new();

    for package_idx in 0..state.parse.package_count() {
        let package = PackageSlot(package_idx);
        let Some(parse_package) = state.parse.package(package_idx) else {
            continue;
        };
        if !state.body_ir_policy.should_lower_package(parse_package) {
            continue;
        }

        let Some(body_ir) = state.body_ir.resident_package(package) else {
            continue;
        };
        if body_ir
            .targets()
            .iter()
            .all(|target| target.coverage().is_complete())
        {
            continue;
        }

        packages.push(package);
    }

    packages
}

/// Keep only requested resident packages that still need complete deferred payloads.
fn packages_needing_finished_split_indexing(
    state: &ProjectState,
    packages: &[PackageSlot],
) -> Vec<PackageSlot> {
    let mut needed = UniqueVec::new();

    for &package in packages {
        let Some(body_ir) = state.body_ir.resident_package(package) else {
            continue;
        };
        if body_ir
            .targets()
            .iter()
            .all(|target| target.coverage().is_complete())
        {
            continue;
        }

        needed.push(package);
    }

    needed.into_vec()
}

/// Return packages from this set that have become complete after a partial rebuild.
fn finished_resident_packages(state: &ProjectState, packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut finished = Vec::new();

    for &package in packages {
        let Some(body_ir) = state.body_ir.resident_package(package) else {
            continue;
        };
        if body_ir
            .targets()
            .iter()
            .all(|target| target.coverage().is_complete())
        {
            finished.push(package);
        }
    }

    finished
}

/// Preserve already-materialized file bodies when a selected-file rebuild replaces a package.
fn extend_with_materialized_body_files(
    package: PackageSlot,
    body_ir: &PackageBodies,
    files: &mut UniqueVec<BodyIrFile>,
) {
    for target in body_ir.targets() {
        if !target.coverage().is_materialized() {
            continue;
        }
        for body in target.bodies() {
            let source = body.source();
            if source.is_written() {
                files.push(BodyIrFile::new(package, source.file_id));
            }
        }
    }
}

/// Return whether `finished` can replace `current` without losing target coverage.
///
/// Equal coverage is not treated as an improvement: replacing identical packages would create extra
/// churn and can still disturb residency/cache state.
fn body_payload_is_coverage_improvement(current: &PackageBodies, finished: &PackageBodies) -> bool {
    if current.targets().len() != finished.targets().len() {
        return false;
    }

    let mut improved = false;
    for (current, finished) in current.targets().iter().zip(finished.targets()) {
        let current_rank = body_coverage_rank(current.coverage());
        let finished_rank = body_coverage_rank(finished.coverage());
        if finished_rank < current_rank {
            return false;
        }
        if finished_rank > current_rank {
            improved = true;
        }
    }

    improved
}

fn body_coverage_rank(coverage: TargetBodiesCoverage) -> u8 {
    match coverage {
        TargetBodiesCoverage::Missing | TargetBodiesCoverage::SkippedByPolicy => 0,
        TargetBodiesCoverage::Partial => 1,
        TargetBodiesCoverage::Complete => 2,
    }
}
