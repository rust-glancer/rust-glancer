//! Applies package residency decisions to project state.
//!
//! Cache storage primitives know how to encode and locate artifacts. This module owns the higher
//! level lifecycle: deciding which resident packages need durable artifacts, writing them, and then
//! dropping phase data so the project returns to its selected memory profile.

use std::sync::Arc;

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::PackageSlot;
use rg_std::Shrink;

use crate::{
    PackageResidency, ProjectMemoryPurgePoint,
    cache::{PackageCacheBodyUpdateInput, PackageCacheUpdate, PackageCacheWriteInput},
    profile::{BuildMemorySampler, record_build_checkpoint},
};

use super::{package_set::PhasePackageSet, split_indexing, state::ProjectState, update};

/// Planned residency transition for one mutable project snapshot.
pub(crate) struct ResidencyApplication<'a> {
    project: &'a mut ProjectState,
    refresh_source_fingerprints_for: PhasePackageSet,
    packages_to_write: PhasePackageSet,
    packages_to_offload: PhasePackageSet,
}

impl<'a> ResidencyApplication<'a> {
    /// Builds the residency transition for a freshly constructed project.
    ///
    /// Source fingerprints have already been computed for every package in the build phases, and
    /// startup-cache hits may already be non-resident. Therefore we only write artifacts for
    /// offloadable packages whose phase data is actually resident, then offload every package
    /// selected by the residency policy.
    pub(crate) fn fresh(project: &'a mut ProjectState) -> Self {
        let packages_to_offload = Self::offloadable_packages(project)
            .filter(|package| Self::package_can_be_offloaded(project, package));
        // Cache artifacts are the durable backing store for offloadable packages. Resident packages
        // stay in memory and should not pay serialization/write cost until policy asks for it.
        let packages_to_write = packages_to_offload
            .filter(|package| Self::package_artifact_is_writable(project, package));

        Self {
            project,
            refresh_source_fingerprints_for: PhasePackageSet::default(),
            packages_to_write,
            packages_to_offload,
        }
    }

    /// Builds the residency transition after a stable-graph source rebuild.
    ///
    /// Rebuilt packages have fresh resident phase data and stale source fingerprints, while
    /// unchanged dependencies may have been lazily materialized from existing artifacts. Only
    /// rebuilt offloadable packages need fresh artifacts; every offloadable package can be dropped
    /// back to its current cache backing store afterward.
    pub(crate) fn restore(project: &'a mut ProjectState, rebuilt_packages: &[PackageSlot]) -> Self {
        Self {
            refresh_source_fingerprints_for: PhasePackageSet::from_slice(rebuilt_packages),
            packages_to_write: Self::rebuilt_offloadable_packages(project, rebuilt_packages)
                .filter(|package| Self::package_artifact_is_writable(project, package)),
            packages_to_offload: Self::offloadable_packages(project)
                .filter(|package| Self::package_can_be_offloaded(project, package)),
            project,
        }
    }

    /// Invalidates disposable cache state, rebuilds from source, and reapplies residency.
    pub(crate) fn failure_recovery(project: &'a mut ProjectState) -> anyhow::Result<()> {
        project
            .cache_store
            .clear_package_artifacts()
            .context("while attempting to clear package cache artifacts")?;
        update::rebuild_resident_from_source(project)
            .context("while attempting to rebuild resident analysis project from source")?;
        let memory_hooks = Arc::clone(&project.memory_hooks);
        Self::fresh(project)
            .apply()
            .context("while attempting to reapply package cache residency")?;
        memory_hooks.purge(ProjectMemoryPurgePoint::AfterProjectBuild);

        Ok(())
    }

    /// Writes required artifacts and offloads selected packages.
    pub(crate) fn apply(self) -> anyhow::Result<()> {
        self.apply_with_sampler(None)
    }

    /// Writes required artifacts and offloads selected packages, recording transient boundaries.
    pub(crate) fn apply_profiled(self, sampler: &mut BuildMemorySampler) -> anyhow::Result<()> {
        self.apply_with_sampler(Some(sampler))
    }

    fn apply_with_sampler(
        mut self,
        mut sampler: Option<&mut BuildMemorySampler>,
    ) -> anyhow::Result<()> {
        let record_residency_profile = !self.refresh_source_fingerprints_for.is_empty()
            || !self.packages_to_write.is_empty()
            || !self.packages_to_offload.is_empty();

        if !self.refresh_source_fingerprints_for.is_empty() {
            self.refresh_source_fingerprints()
                .context("while attempting to refresh package cache source fingerprints")?;
        }

        if record_residency_profile {
            self.record_project_checkpoint(&mut sampler, "before package cache write");
        }
        self.write_package_artifacts(&self.packages_to_write)?;
        if record_residency_profile {
            self.record_project_checkpoint(&mut sampler, "after package cache write");
        }

        let packages_to_offload = std::mem::take(&mut self.packages_to_offload);
        for package in packages_to_offload.iter() {
            self.offload_package(package)?;
        }
        if record_residency_profile {
            self.record_project_checkpoint(&mut sampler, "after package payload offload");
        }

        self.finish_offloading(&packages_to_offload);
        if record_residency_profile {
            self.record_project_checkpoint(&mut sampler, "after package offload cleanup");
        }
        self.project
            .cache_store
            .cleanup_stale_generations()
            .context("while attempting to clean stale package cache generations")?;

        Ok(())
    }

    fn record_project_checkpoint(
        &self,
        sampler: &mut Option<&mut BuildMemorySampler>,
        label: &'static str,
    ) {
        let Some(sampler) = sampler.as_mut() else {
            return;
        };
        let sampler = &mut **sampler;
        let process_memory = sampler.sample_process_memory();
        let project_bytes = sampler.measure_retained(&*self.project);
        record_build_checkpoint(label, project_bytes, project_bytes, process_memory);
    }

    /// Returns all packages selected by the current residency policy.
    fn offloadable_packages(project: &ProjectState) -> PhasePackageSet {
        PhasePackageSet::from_packages(
            (0..project.workspace.packages().len())
                .map(PackageSlot)
                .filter(|package| {
                    project.package_residency.package(*package)
                        == Some(PackageResidency::Offloadable)
                })
                .collect(),
        )
    }

    /// Intersects the rebuilt package set with the current offloadable package set.
    fn rebuilt_offloadable_packages(
        project: &ProjectState,
        rebuilt_packages: &[PackageSlot],
    ) -> PhasePackageSet {
        let package_count = project.workspace.packages().len();
        let mut rebuilt = vec![false; package_count];
        for package in rebuilt_packages {
            if package.0 < package_count {
                rebuilt[package.0] = true;
            }
        }
        let mut packages = Vec::new();
        for (package_idx, was_rebuilt) in rebuilt.iter().copied().enumerate() {
            let package = PackageSlot(package_idx);
            if was_rebuilt
                && project.package_residency.package(package) == Some(PackageResidency::Offloadable)
            {
                packages.push(package);
            }
        }
        PhasePackageSet::from_packages(packages)
    }

    /// Refreshes source fingerprints for packages that were rebuilt from source.
    fn refresh_source_fingerprints(&mut self) -> anyhow::Result<()> {
        self.project.cache_plan.refresh_source_fingerprints(
            self.project.workspace.workspace_root(),
            &self.project.parse,
            &mut self.project.package_source_fingerprints,
            self.refresh_source_fingerprints_for.as_slice(),
        )
    }

    /// Return whether this package can produce a coherent replacement artifact.
    ///
    /// A normal build writes three resident phases. An exact Body IR rebuild instead keeps both
    /// declaration phases offloaded and copies their encoded sections from the prior artifact.
    fn package_artifact_is_writable(project: &ProjectState, package: PackageSlot) -> bool {
        if !split_indexing::package_deferred_payload_is_durable(project, package) {
            return false;
        }
        let Some(body_ir) = project.body_ir.resident_package(package) else {
            return false;
        };
        let declarations_resident = project.def_map.resident_package(package).is_some()
            && project.semantic_ir.resident_package(package).is_some();
        let declarations_offloaded = project.def_map.package_is_offloaded(package)
            && project.semantic_ir.package_is_offloaded(package)
            && body_ir.has_cached_payloads();
        declarations_resident || declarations_offloaded
    }

    /// Returns whether dropping this package would leave every resident value durably backed.
    ///
    /// A package with no resident phase data is already offloaded. If any phase is resident, the
    /// package must be writable as one coherent artifact before residency can release it.
    fn package_can_be_offloaded(project: &ProjectState, package: PackageSlot) -> bool {
        let has_resident_payload = project.def_map.resident_package(package).is_some()
            || project.semantic_ir.resident_package(package).is_some()
            || project.body_ir.resident_package(package).is_some();

        !has_resident_payload || Self::package_artifact_is_writable(project, package)
    }

    /// Writes every replacement artifact before any selected resident payload is dropped.
    ///
    /// A replacement may encode all three resident phases or copy offloaded declaration sections
    /// while updating Body IR. Package-local writes can run in parallel, but the package-set marker
    /// is committed only after every artifact succeeds.
    fn write_package_artifacts(&self, packages: &PhasePackageSet) -> anyhow::Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        let update = self.project.cache_store.begin_artifact_update()?;
        let thread_pool = Self::local_thread_pool("rg-cache-write")?;
        let project = &*self.project;

        // Artifact serialization is package-local and usually more expensive than the final state
        // mutation. Write every durable artifact first; only then can callers safely drop residents.
        thread_pool
            .install(|| {
                packages.as_slice().par_iter().try_for_each(|package| {
                    Self::write_package_artifact(project, &update, *package)
                })
            })
            .context("while attempting to write package cache artifacts")?;
        update
            .commit()
            .context("while attempting to commit package cache artifact update")
    }

    /// Drops compactable project data after package payloads have been offloaded.
    fn finish_offloading(&mut self, offloaded_packages: &PhasePackageSet) {
        if !offloaded_packages.is_empty() {
            // Offloading drops many strong `Name` handles from phase payloads. Prune the interner
            // immediately so dead weak entries and their Arc control blocks do not pin allocator
            // pages until a later rebuild happens to compact the project.
            Shrink::shrink_to_fit(&mut self.project.names);

            // File ids and paths remain resident as the source inventory. Line indexes are larger
            // and can be reconstructed from saved source text when a query needs LSP coordinates.
            let offloaded_package_indices = offloaded_packages.package_indices();
            self.project
                .parse
                .offload_line_indexes_for_packages(&offloaded_package_indices);
        }
    }

    /// Writes one coherent package artifact from the phase data available at this boundary.
    ///
    /// Jointly resident declaration phases are encoded normally and may reuse untouched Body shards.
    /// If both declaration phases are offloaded, an exact Body rebuild instead copies their encoded
    /// sections from the pinned prior revision. A mixed declaration residency state is rejected.
    fn write_package_artifact(
        project: &ProjectState,
        update: &PackageCacheUpdate<'_>,
        package: PackageSlot,
    ) -> anyhow::Result<()> {
        let header = project
            .cache_plan
            .artifact_header(package, &project.package_source_fingerprints)
            .with_context(|| {
                format!(
                    "while attempting to build package cache header for package {}",
                    package.0,
                )
            })?;
        let parse = project.parse.package(package.0).with_context(|| {
            format!(
                "while attempting to fetch parsed package {} for cache artifact",
                package.0,
            )
        })?;
        let body_ir = project.body_ir.resident_package(package).with_context(|| {
            format!(
                "while attempting to fetch resident body IR package {}",
                package.0,
            )
        })?;

        let parse = parse.parse_snapshot().with_context(|| {
            format!(
                "while attempting to snapshot parse metadata for package {}",
                package.0,
            )
        })?;

        let def_map = project.def_map.resident_package(package);
        let semantic_ir = project.semantic_ir.resident_package(package);
        let write = match (def_map, semantic_ir) {
            (Some(def_map), Some(semantic_ir)) => {
                let input =
                    PackageCacheWriteInput::new(&header, &parse, def_map, semantic_ir, body_ir);
                if body_ir.has_cached_payloads() {
                    let reader = project
                        .cache_store
                        .open_artifact(&header)
                        .with_context(|| {
                            format!(
                                "while attempting to open prior cache artifact for package {}",
                                package.0,
                            )
                        })?
                        .with_context(|| {
                            format!(
                                "prior cache artifact is missing for package {} with cached Body IR payloads",
                                package.0,
                            )
                        })?;
                    update.write_input_reusing_cached_body_ir(input, &reader)
                } else {
                    update.write_input(input)
                }
            }
            (None, None) => {
                // Exact target materialization starts from a fully offloaded artifact. It restores
                // a manifest-backed Body IR package and rebuilds only the requested target;
                // declaration reads belong to short-lived transactions, so DefMap and Semantic IR
                // remain offloaded. Cached Body placeholders distinguish that rewrite overlay from
                // an arbitrary mixed-residency state.
                anyhow::ensure!(
                    project.def_map.package_is_offloaded(package)
                        && project.semantic_ir.package_is_offloaded(package),
                    "package {} declaration phases are neither jointly resident nor offloaded",
                    package.0,
                );
                anyhow::ensure!(
                    body_ir.has_cached_payloads(),
                    "package {} needs cached declaration sections without cached Body IR siblings",
                    package.0,
                );

                // Pin the old revision before the atomic replacement. The changed Body IR is
                // encoded normally; sibling Body shards and both declaration sections are copied.
                let input = PackageCacheBodyUpdateInput::new(&header, &parse, body_ir);
                let reader = project
                    .cache_store
                    .open_artifact(&header)
                    .with_context(|| {
                        format!(
                            "while attempting to open prior cache artifact for package {}",
                            package.0,
                        )
                    })?
                    .with_context(|| {
                        format!(
                            "prior cache artifact is missing for package {} with cached declarations",
                            package.0,
                        )
                    })?;
                update.write_body_update_reusing_cached_sections(input, &reader)
            }
            _ => anyhow::bail!(
                "package {} has inconsistent DefMap and Semantic IR residency",
                package.0,
            ),
        };

        write.with_context(|| {
            format!(
                "while attempting to write package cache artifact for package {}",
                package.0,
            )
        })
    }

    /// Offloads one package from every artifact-backed phase database.
    fn offload_package(&mut self, package: PackageSlot) -> anyhow::Result<()> {
        // Only drop resident data after the full cross-phase package artifact is durable. If a
        // future implementation downgrades write errors to warnings, this invariant should remain.
        self.project
            .def_map
            .offload_package(package)
            .with_context(|| {
                format!("while attempting to offload def-map package {}", package.0)
            })?;
        self.project
            .semantic_ir
            .offload_package(package)
            .with_context(|| {
                format!(
                    "while attempting to offload semantic IR package {}",
                    package.0
                )
            })?;
        self.project
            .body_ir
            .offload_package(package)
            .with_context(|| {
                format!("while attempting to offload body IR package {}", package.0)
            })?;

        Ok(())
    }

    /// Creates a short-lived Rayon pool for package artifact serialization.
    fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
        rayon::ThreadPoolBuilder::new()
            .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
            .build()
            .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
    }
}
