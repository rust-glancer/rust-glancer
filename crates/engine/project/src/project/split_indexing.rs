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
//!   hover or a mix of files and crates for reference search.
//! - `DetachedSplitIndexing` and `merge_finished` let an LSP background clone finish deferred
//!   payloads, publish priority packages early, and merge those improvements into the saved project.
//!
//! That last path has to be monotonic. Query-time materialization may finish a package before the
//! background clone finishes, so a stale background result must not replace better saved coverage
//! with an older partial view.

use anyhow::Context as _;
use rg_body_ir::{BodyIrFile, CrateBodiesCoverage, PackageBodies};
use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
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
/// File surfaces are used when a query is tied to one semantic interpretation of a concrete
/// source file. Crate surfaces prepare complete bodies for exactly the listed semantic crates.
/// Reference search can need both shapes when a text prefilter narrows some work to files while
/// another part of the query still needs whole-crate coverage.
#[derive(Debug, Clone, Copy)]
pub enum AnalysisSurface<'a> {
    /// Prepare deferred data needed by analysis rooted in these crate-local source files.
    Files(&'a [(CrateRef, FileId)]),
    /// Prepare complete deferred data for these exact semantic crates.
    Crates(&'a [CrateRef]),
    /// Prepare exact file coverage first, then complete the listed crates.
    FilesAndCrates {
        files: &'a [(CrateRef, FileId)],
        crates: &'a [CrateRef],
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
/// - `finish` is the background path. It completes the remaining deferred payloads chosen by the
///   project's configured indexing policy and then reapplies package residency.
///
/// Detached finishing uses the same policy, but runs through `DetachedSplitIndexing` so callers
/// cannot accidentally treat a project clone as an ordinary live project. The clone stays hidden
/// behind that capability while `FinishedSplitIndexing` package improvements are published back
/// into the saved project.
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

    /// Returns whether preparing the requested query surface would replace retained state.
    ///
    /// Offloaded packages do not need materialization because query transactions can read their
    /// durable artifacts without replacing retained project state.
    pub fn needs_materialization(&self, surface: AnalysisSurface<'_>) -> bool {
        match surface {
            AnalysisSurface::Files(files) => files.iter().any(|&(crate_ref, file)| {
                body_file_needs_materialization(&self.project.state, crate_ref, file)
            }),
            AnalysisSurface::Crates(crates) => crates
                .iter()
                .any(|&crate_ref| crate_needs_materialization(&self.project.state, crate_ref)),
            AnalysisSurface::FilesAndCrates { files, crates } => {
                self.needs_materialization(AnalysisSurface::Files(files))
                    || self.needs_materialization(AnalysisSurface::Crates(crates))
            }
        }
    }

    /// Materialize deferred analysis data for the requested query surface.
    pub fn materialize(&mut self, surface: AnalysisSurface<'_>) -> anyhow::Result<()> {
        materialize_surface(&mut self.project.state, surface)
    }

    /// Merge a detached finish result, returning whether it improved the saved project.
    pub fn merge_finished(&mut self, finished: FinishedSplitIndexing) -> anyhow::Result<bool> {
        merge_finished_project(&mut self.project.state, finished)
    }
}

/// Owned capability for finishing split indexing away from the saved project.
///
/// The detached project is deliberately private. Background workers only need to finish deferred
/// indexing and hand the result back to the saved project; exposing the raw clone would make it too
/// easy to bypass generation checks or add query shortcuts that do not see live saved-state
/// mutations.
#[must_use]
#[derive(Debug, MemorySize)]
pub struct DetachedSplitIndexing {
    project: Project,
}

impl DetachedSplitIndexing {
    pub(super) fn new(project: Project) -> Self {
        Self { project }
    }

    /// Return the packages still selected by the configured deferred-indexing policy.
    pub fn unfinished_packages(&self) -> Vec<PackageSlot> {
        unfinished_split_indexing_packages(&self.project.state)
    }

    /// Return package slots whose parsed source inventory contains this path.
    ///
    /// Editor paths do not have to use the same spelling as Cargo metadata paths, so the lookup
    /// canonicalizes before consulting the detached project's package-local file tables.
    pub fn package_slots_for_path(
        &self,
        path: &std::path::Path,
    ) -> anyhow::Result<Vec<PackageSlot>> {
        package_slots_for_path(&self.project.state, path)
    }

    /// Finish all deferred work while publishing priority packages as soon as they resolve.
    ///
    /// Every package still belongs to one Body IR build. The callback is an early copy-out point,
    /// not another build boundary, so the remaining background work keeps its ordinary parallel
    /// scheduling and shared read transactions.
    pub fn finish_with_package_priority(
        self,
        priority_packages: impl Fn() -> Vec<PackageSlot> + Sync,
        publish_priority: impl Fn(FinishedSplitIndexing) + Sync,
    ) -> anyhow::Result<FinishedSplitIndexing> {
        self.finish_with_optional_package_priority(Some(&priority_packages), &publish_priority)
    }

    /// Finish every remaining package inside the detached project clone.
    pub fn finish(self) -> anyhow::Result<FinishedSplitIndexing> {
        self.finish_with_optional_package_priority(None, &|_| {})
    }

    fn finish_with_optional_package_priority(
        mut self,
        priority_packages: Option<&(dyn Fn() -> Vec<PackageSlot> + Sync)>,
        publish_priority: &(dyn Fn(FinishedSplitIndexing) + Sync),
    ) -> anyhow::Result<FinishedSplitIndexing> {
        let packages = unfinished_split_indexing_packages(&self.project.state);
        let publish_package = |package, bodies| {
            publish_priority(FinishedSplitIndexing {
                packages: vec![(package, bodies)],
            });
        };
        let mut sampler = BuildMemorySampler::disabled();
        finish_resident_packages_with_sampler(
            &mut self.project.state,
            &packages,
            priority_packages,
            &publish_package,
            &mut sampler,
        )
        .context("while attempting to finish detached deferred packages")?;
        capture_finished_packages(&self.project.state, &packages)
    }
}

pub(super) fn package_slots_for_path(
    state: &ProjectState,
    path: &std::path::Path,
) -> anyhow::Result<Vec<PackageSlot>> {
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("while attempting to canonicalize {}", path.display()))?;
    let mut packages = state
        .parse
        .file_refs_for_path(&canonical_path)
        .into_iter()
        .map(|file| PackageSlot(file.package))
        .collect::<Vec<_>>();
    packages.sort_unstable();
    packages.dedup();
    Ok(packages)
}

/// Package payloads finished inside a detached project clone.
///
/// A background worker can publish one of these values while its detached build continues through
/// later packages. The saved project still decides package by package whether each payload is an
/// improvement over on-demand work that happened in parallel.
#[derive(Debug, Clone, MemorySize)]
pub struct FinishedSplitIndexing {
    packages: Vec<(PackageSlot, PackageBodies)>,
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
        AnalysisSurface::Crates(crates) => materialize_crates(state, crates),
        AnalysisSurface::FilesAndCrates { files, crates } => {
            materialize_files(state, files)?;
            materialize_crates(state, crates)
        }
    }
}

/// Merge a detached finish result, returning whether it improved the saved project.
fn merge_finished_project(
    state: &mut ProjectState,
    finished: FinishedSplitIndexing,
) -> anyhow::Result<bool> {
    let packages = merge_finished_packages(state, finished.packages)
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
/// For this split-indexing mode the deferred payload is Body IR. Complete crate coverage is
/// durable, and policy-skipped secondary targets are durable when their target kind agrees with
/// the configured eager policy. Partial selected-file coverage is intentionally transient: writing
/// it as a package artifact would make future startup-cache reads look complete while silently
/// missing bodies that were never materialized.
pub(crate) fn package_deferred_payload_is_durable(
    state: &ProjectState,
    package: PackageSlot,
) -> bool {
    let Some(body_ir) = state.body_ir.resident_package(package) else {
        return false;
    };

    configured_package_is_finished(state, package, body_ir)
}

/// Materialize the deferred payload for a set of source files.
///
/// This is the narrow on-demand path used by file-local LSP queries. The deferred payload is stored
/// as package-shaped Body IR, so the rebuild keeps already-materialized files for the same exact
/// crate interpretation as well as the newly requested files.
fn materialize_files(state: &mut ProjectState, files: &[(CrateRef, FileId)]) -> anyhow::Result<()> {
    // A cached package cannot retain a partial resident target beside lazy sibling shards. Promote
    // only the requested target interpretations to complete coverage, rewrite the artifact, and
    // return the package to lazy residency before handling ordinary resident selected-file work.
    // This keeps the manifest-only overlay inside one synchronous lifecycle transition.
    let cached_crates = files
        .iter()
        .map(|&(crate_ref, _)| crate_ref)
        .filter(|&crate_ref| {
            state.body_ir.package_is_offloaded(crate_ref.package)
                && crate_needs_materialization(state, crate_ref)
        })
        .collect::<UniqueVec<_>>();
    if !cached_crates.is_empty() {
        materialize_crates(state, cached_crates.as_slice()).context(
            "while attempting to complete cached targets requested by file materialization",
        )?;
    }

    let mut body_files = files
        .iter()
        .filter(|&&(crate_ref, file)| body_file_needs_materialization(state, crate_ref, file))
        .map(|&(crate_ref, file)| BodyIrFile::new(crate_ref, file))
        .collect::<UniqueVec<_>>();
    if body_files.is_empty() {
        return Ok(());
    }

    // Selected-file rebuilds replace a package payload. Keep already-materialized files selected so
    // preparing one new file cannot accidentally discard earlier on-demand coverage from the same
    // package.
    let requested_packages = PhasePackageSet::from_body_files(body_files.as_slice());
    restore_offloaded_packages_for_body_rebuild(state, requested_packages.as_slice())?;
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
        .worker_limit(state.indexing_preference.body_ir_worker_limit())
        .selected_files(body_files.into_vec())
        .build();

    // Body lowering reparses evicted files and repopulates their source entries. The text is not
    // part of retained deferred analysis, so release it before either publishing the payload or
    // returning a build error to a direct Project API caller.
    state.parse.evict_saved_source_text();
    let body_ir = body_ir.context("while attempting to materialize deferred analysis for files")?;

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

/// Return whether a file-local query still needs source rebuilding before it can run.
///
/// Non-resident Body IR packages are already backed by durable cache artifacts. Query transactions
/// can lazy-load them much more cheaply than rebuilding the package from source, so they are ready
/// for this on-demand path. Resident complete packages are also ready. Partial packages only need a
/// rebuild when the requested file's bodies are not among the already-materialized bodies.
fn body_file_needs_materialization(
    state: &ProjectState,
    crate_ref: CrateRef,
    file: FileId,
) -> bool {
    let Some(body_ir) = state.body_ir.resident_package(crate_ref.package) else {
        return crate_needs_materialization(state, crate_ref);
    };
    let Some(crate_bodies) = body_ir.crate_bodies(crate_ref.crate_id) else {
        return true;
    };
    if crate_bodies.coverage().is_complete() {
        return false;
    }

    !crate_bodies
        .bodies()
        .iter()
        .any(|body| body.source().is_written() && body.source().file_id == file)
}

/// Materialize complete bodies for exactly the requested semantic crates.
///
/// For a package with a library and many test targets, requesting one test replaces only that
/// test's crate slot. If the package was offloaded, sibling targets remain as cached manifest
/// placeholders until the package artifact is rewritten with their existing encoded shards.
fn materialize_crates(state: &mut ProjectState, crates: &[CrateRef]) -> anyhow::Result<()> {
    let crates = crates
        .iter()
        .copied()
        .filter(|&crate_ref| crate_needs_materialization(state, crate_ref))
        .collect::<UniqueVec<_>>();
    if crates.is_empty() {
        return Ok(());
    }

    let packages = PhasePackageSet::from_crates(crates.as_slice());
    restore_offloaded_packages_for_body_rebuild(state, packages.as_slice())?;
    let rebuild_subset = packages.visible_dependency_subset(&state.workspace);
    let loaders = PackageReadLoaders::new(state);
    let body_ir = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            packages.as_slice(),
            &mut state.names,
            loaders.def_map,
            loaders.semantic_ir,
            &rebuild_subset,
        )
        .worker_limit(state.indexing_preference.body_ir_worker_limit())
        .selected_crates(crates)
        .build();

    // Package materialization has the same source lifetime as file-local materialization. Keep
    // only the derived Body IR after the builder finishes, including on its error path.
    state.parse.evict_saved_source_text();
    let body_ir =
        body_ir.context("while attempting to materialize complete deferred analysis for crates")?;

    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);
    let finished_packages = finished_resident_packages(state, packages.as_slice());
    apply_finished_residency(state, &finished_packages).context(
        "while attempting to apply residency after preparing complete deferred analysis for crates",
    )?;
    Ok(())
}

/// Complete the deferred packages selected by the project's configured payload policy.
fn finish_resident_with_sampler(
    state: &mut ProjectState,
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<Vec<PackageSlot>> {
    let packages = unfinished_split_indexing_packages(state);
    finish_resident_packages_with_sampler(state, &packages, None, &|_, _| {}, sampler)
        .context("while attempting to finish resident deferred packages")?;
    Ok(packages)
}

/// Complete the already-normalized deferred package set in one Body IR build.
fn finish_resident_packages_with_sampler(
    state: &mut ProjectState,
    packages: &[PackageSlot],
    priority_packages: Option<&(dyn Fn() -> Vec<PackageSlot> + Sync)>,
    publish_priority: &(dyn Fn(PackageSlot, PackageBodies) + Sync),
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let finish_subset =
        PhasePackageSet::from_slice(packages).visible_dependency_subset(&state.workspace);
    let loaders = PackageReadLoaders::new(state);
    let rebuilder = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            packages,
            &mut state.names,
            loaders.def_map,
            loaders.semantic_ir,
            &finish_subset,
        )
        .worker_limit(state.indexing_preference.body_ir_worker_limit())
        .configured_bodies(state.body_ir_policy);
    let body_ir = match priority_packages {
        Some(priority_packages) => {
            rebuilder.build_with_package_priority(priority_packages, publish_priority)
        }
        None => rebuilder.build(),
    };

    // Fresh construction evicts this same reloadable text before exposing retained-state memory.
    // Deferred finishing must do so before its purge and checkpoints as well; otherwise source
    // allocations keep their allocator pages live for the rest of the process.
    state.parse.evict_saved_source_text();
    let body_ir = body_ir.context("while attempting to finish deferred indexing packages")?;

    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);
    state
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterBodyIrBuild);
    record_project_checkpoint(state, sampler, "after deferred indexing");

    Ok(())
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

/// Copy finished packages out of the detached project for final publication.
fn capture_finished_packages(
    state: &ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<FinishedSplitIndexing> {
    let mut finished = Vec::with_capacity(packages.len());
    for &package in packages {
        let bodies = state
            .body_ir
            .resident_package(package)
            .with_context(|| {
                format!(
                    "while attempting to read finished deferred payload package {}",
                    package.0,
                )
            })?
            .clone();
        finished.push((package, bodies));
    }
    Ok(FinishedSplitIndexing { packages: finished })
}

/// Merge only package-wise coverage improvements from detached background work.
fn merge_finished_packages(
    state: &mut ProjectState,
    finished: Vec<(PackageSlot, PackageBodies)>,
) -> anyhow::Result<Vec<PackageSlot>> {
    let mut replacements = Vec::new();

    // Decide every replacement before mutating the saved project. Query-time preparation may have
    // finished a package while the detached build was running, in which case equal or older
    // coverage must not replace it.
    for (package, finished_bodies) in finished {
        // An offloaded slot already has a durable package artifact. A priority publication can
        // reach that state before the final detached result returns the same package, and putting
        // the duplicate back would turn a deliberately lazy slot into retained Body IR again.
        if state.body_ir.package_is_offloaded(package) {
            continue;
        }

        let replacement = match state.body_ir.resident_package(package) {
            Some(current_bodies) => {
                merge_body_payload_improvements(current_bodies, &finished_bodies)
            }
            None => Some(finished_bodies),
        };

        // The background clone can lag behind query-time preparation. Merge improvements one
        // semantic crate at a time so its newly completed primary target cannot downgrade an
        // on-demand test target, or vice versa.
        if let Some(replacement) = replacement {
            replacements.push((package, replacement));
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
        let Some(body_ir) = state.body_ir.resident_package(package) else {
            continue;
        };
        if configured_package_is_finished(state, package, body_ir) {
            continue;
        }

        packages.push(package);
    }

    packages
}

/// Return packages from this set that have become complete after a partial rebuild.
fn finished_resident_packages(state: &ProjectState, packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut finished = Vec::new();

    for &package in packages {
        let Some(body_ir) = state.body_ir.resident_package(package) else {
            continue;
        };
        if configured_package_is_finished(state, package, body_ir) {
            finished.push(package);
        }
    }

    finished
}

/// Return whether every semantic crate in a package satisfies the eager-target policy.
fn configured_package_is_finished(
    state: &ProjectState,
    package: PackageSlot,
    bodies: &PackageBodies,
) -> bool {
    bodies
        .crates()
        .iter()
        .enumerate()
        .all(|(crate_idx, crate_bodies)| {
            configured_crate_is_finished(
                state,
                CrateRef {
                    package,
                    crate_id: rg_ir_model::CrateId(crate_idx),
                },
                crate_bodies.coverage(),
            )
        })
}

/// Return whether one crate satisfies the project's configured eager-target policy.
///
/// An explicitly materialized secondary target is complete and therefore durable. A still
/// deferred secondary target is also finished for ordinary background indexing, but only when its
/// `SkippedByPolicy` marker agrees with the Cargo target kind.
fn configured_crate_is_finished(
    state: &ProjectState,
    crate_ref: CrateRef,
    coverage: CrateBodiesCoverage,
) -> bool {
    if coverage.is_complete() {
        return true;
    }
    if !matches!(coverage, CrateBodiesCoverage::SkippedByPolicy) {
        return false;
    }

    let Some(parse_package) = state.parse.package(crate_ref.package.0) else {
        return false;
    };
    // Semantic crate ids are allocated from Cargo targets in their parse-package order. Use that
    // structural identity here so residency checks do not need to load an offloaded DefMap merely
    // to validate a deliberate secondary-target skip.
    let Some(parse_target) = parse_package.targets().get(crate_ref.crate_id.0) else {
        return false;
    };

    !state
        .body_ir_policy
        .should_lower_target(parse_package, parse_target)
}

/// Return whether an exact crate request still lacks complete Body IR.
fn crate_needs_materialization(state: &ProjectState, crate_ref: CrateRef) -> bool {
    if let Some(crate_bodies) = state
        .body_ir
        .resident_package(crate_ref.package)
        .and_then(|package| package.crate_bodies(crate_ref.crate_id))
    {
        return !crate_bodies.coverage().is_complete();
    }
    if !state.body_ir.package_is_offloaded(crate_ref.package) {
        return true;
    }

    cached_crate_coverage(state, crate_ref)
        .map(|coverage| !coverage.is_complete())
        // A missing or corrupt artifact still needs the materialization path so it can report the
        // typed cache error instead of silently running with absent bodies.
        .unwrap_or(true)
}

fn cached_crate_coverage(state: &ProjectState, crate_ref: CrateRef) -> Option<CrateBodiesCoverage> {
    let cached_package = state.cache_plan.package(crate_ref.package)?;
    state
        .cache_store
        .read_probe_for_package(cached_package)
        .ok()??
        .body_ir_coverage
        .get(crate_ref.crate_id.0)
        .copied()
}

/// Restore the package containers needed to rebuild one target from an offloaded package.
///
/// DefMap and Semantic IR are restored as ordinary package payloads because Body IR rebuilding
/// reads them. Body IR restores only crate manifests: the builder replaces requested crate slots,
/// and the cache writer later copies untouched sibling shards from the old artifact. The temporary
/// mixed package is never published to ordinary Body IR queries.
fn restore_offloaded_packages_for_body_rebuild(
    state: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    let offloaded = packages
        .iter()
        .copied()
        .filter(|&package| state.body_ir.package_is_offloaded(package))
        .collect::<Vec<_>>();
    if offloaded.is_empty() {
        return Ok(());
    }

    let loaders = PackageReadLoaders::new(state);
    // Decode every requested package before mutating project state. A corrupt later artifact must
    // not leave an earlier package half-restored when the exact materialization request fails.
    let restored = offloaded
        .into_iter()
        .map(|package| {
            loaders
                .load_package_payloads(package)
                .map(|payloads| (package, payloads))
                .with_context(|| {
                    format!(
                        "restore offloaded package {} for exact target materialization",
                        package.0
                    )
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    for (package, (def_map, semantic_ir, bodies)) in restored {
        state
            .def_map
            .replace_package(package, def_map)
            .context("offloaded DefMap package slot should exist")?;
        state
            .semantic_ir
            .replace_package(package, semantic_ir)
            .context("offloaded Semantic IR package slot should exist")?;
        state
            .body_ir
            .replace_package(package, bodies)
            .context("offloaded Body IR package slot should exist")?;
    }
    Ok(())
}

/// Preserve already-materialized file bodies when a selected-file rebuild replaces a package.
fn extend_with_materialized_body_files(
    package: PackageSlot,
    body_ir: &PackageBodies,
    files: &mut UniqueVec<BodyIrFile>,
) {
    for (crate_idx, crate_bodies) in body_ir.crates().iter().enumerate() {
        if !crate_bodies.coverage().is_materialized() {
            continue;
        }
        let crate_ref = CrateRef {
            package,
            crate_id: rg_ir_model::CrateId(crate_idx),
        };
        for body in crate_bodies.bodies() {
            let source = body.source();
            if source.is_written() {
                files.push(BodyIrFile::new(crate_ref, source.file_id));
            }
        }
    }
}

/// Merge the better payload for each semantic crate without downgrading sibling target coverage.
///
/// Equal coverage keeps the saved payload. Besides avoiding churn, this preserves query-time body
/// data when a detached build reaches the same coverage through an older source snapshot.
fn merge_body_payload_improvements(
    current: &PackageBodies,
    finished: &PackageBodies,
) -> Option<PackageBodies> {
    if current.crates().len() != finished.crates().len() {
        return None;
    }

    let mut improved = false;
    let crates = current
        .crates()
        .iter()
        .zip(finished.crates())
        .map(|(current, finished)| {
            if body_coverage_rank(finished.coverage()) > body_coverage_rank(current.coverage()) {
                improved = true;
                finished.clone()
            } else {
                current.clone()
            }
        })
        .collect();

    improved.then(|| PackageBodies::new(crates))
}

fn body_coverage_rank(coverage: CrateBodiesCoverage) -> u8 {
    match coverage {
        CrateBodiesCoverage::Missing | CrateBodiesCoverage::SkippedByPolicy => 0,
        CrateBodiesCoverage::Partial => 1,
        CrateBodiesCoverage::Complete => 2,
    }
}
