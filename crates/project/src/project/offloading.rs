//! Applies package residency decisions to project state.
//!
//! Cache storage primitives know how to encode and locate artifacts. This module owns the higher
//! level lifecycle: deciding which resident packages need durable artifacts, writing them, and then
//! dropping phase data so the project returns to its selected memory profile.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_body_ir::BodyIrPackageBundle;
use rg_def_map::{DefMapPackageBundle, PackageSlot};
use rg_semantic_ir::SemanticIrPackageBundle;

use crate::{
    PackageResidency,
    cache::{PackageCacheArtifact, PackageCacheBodyIrState, PackageCachePayload},
};

use super::{state::ProjectState, update};

/// Planned residency transition for one mutable project snapshot.
pub(crate) struct ResidencyApplication<'a> {
    project: &'a mut ProjectState,
    refresh_source_fingerprints_for: Vec<PackageSlot>,
    packages_to_write: Vec<PackageSlot>,
    packages_to_offload: Vec<PackageSlot>,
}

impl<'a> ResidencyApplication<'a> {
    /// Builds the residency transition for a freshly constructed project.
    ///
    /// Source fingerprints have already been computed for every package in the build phases, and
    /// startup-cache hits may already be non-resident. Therefore we only write artifacts for
    /// offloadable packages whose phase data is actually resident, then offload every package
    /// selected by the residency policy.
    pub(crate) fn fresh(project: &'a mut ProjectState) -> Self {
        let packages_to_offload = Self::offloadable_packages(project);
        // Cache artifacts are the durable backing store for offloadable packages. Resident packages
        // stay in memory and should not pay serialization/write cost until policy asks for it.
        let packages_to_write = packages_to_offload
            .iter()
            .copied()
            .filter(|package| Self::package_artifact_is_resident(project, *package))
            .collect::<Vec<_>>();

        Self {
            project,
            refresh_source_fingerprints_for: Vec::new(),
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
            refresh_source_fingerprints_for: rebuilt_packages.to_vec(),
            packages_to_write: Self::rebuilt_offloadable_packages(project, rebuilt_packages),
            packages_to_offload: Self::offloadable_packages(project),
            project,
        }
    }

    /// Invalidates disposable cache state, rebuilds from source, and reapplies residency.
    pub(crate) fn failure_recovery(project: &'a mut ProjectState) -> anyhow::Result<()> {
        project
            .cache_store
            .invalidate_workspace_cache()
            .context("while attempting to invalidate package cache namespace")?;
        update::rebuild_resident_from_source(project)
            .context("while attempting to rebuild resident analysis project from source")?;
        Self::fresh(project)
            .apply()
            .context("while attempting to reapply package cache residency")?;

        Ok(())
    }

    /// Writes required artifacts and offloads selected packages.
    pub(crate) fn apply(mut self) -> anyhow::Result<()> {
        if !self.refresh_source_fingerprints_for.is_empty() {
            self.refresh_source_fingerprints()
                .context("while attempting to refresh package cache source fingerprints")?;
        }

        self.write_package_artifacts(&self.packages_to_write)?;

        let mut offloaded_packages = Vec::new();
        let packages_to_offload = std::mem::take(&mut self.packages_to_offload);
        for package in packages_to_offload {
            self.offload_package(package)?;
            offloaded_packages.push(package.0);
        }

        self.finish_offloading(&offloaded_packages);
        self.project
            .cache_store
            .cleanup_stale_generations()
            .context("while attempting to clean stale package cache generations")?;

        Ok(())
    }

    /// Returns all packages selected by the current residency policy.
    fn offloadable_packages(project: &ProjectState) -> Vec<PackageSlot> {
        (0..project.workspace.packages().len())
            .map(PackageSlot)
            .filter(|package| {
                project.package_residency.package(*package) == Some(PackageResidency::Offloadable)
            })
            .collect::<Vec<_>>()
    }

    /// Intersects the rebuilt package set with the current offloadable package set.
    fn rebuilt_offloadable_packages(
        project: &ProjectState,
        rebuilt_packages: &[PackageSlot],
    ) -> Vec<PackageSlot> {
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
        packages
    }

    /// Refreshes source fingerprints for packages that were rebuilt from source.
    fn refresh_source_fingerprints(&mut self) -> anyhow::Result<()> {
        self.project.cache_plan.refresh_source_fingerprints(
            self.project.workspace.workspace_root(),
            &self.project.parse,
            &mut self.project.package_source_fingerprints,
            &self.refresh_source_fingerprints_for,
        )
    }

    /// Returns whether all artifact-backed phase payloads are resident for this package.
    fn package_artifact_is_resident(project: &ProjectState, package: PackageSlot) -> bool {
        project.def_map.resident_package(package).is_some()
            && project.semantic_ir.resident_package(package).is_some()
            && project.body_ir.resident_package(package).is_some()
    }

    /// Writes durable cache artifacts for packages whose resident payloads are about to be dropped.
    fn write_package_artifacts(&self, packages: &[PackageSlot]) -> anyhow::Result<()> {
        if packages.len() <= 1 {
            for package in packages {
                Self::write_package_artifact(self.project, *package)?;
            }
            return Ok(());
        }

        let thread_pool = Self::local_thread_pool("rg-cache-write")?;
        let project = &*self.project;

        // Artifact serialization is package-local and usually more expensive than the final state
        // mutation. Write every durable artifact first; only then can callers safely drop residents.
        thread_pool
            .install(|| {
                packages
                    .par_iter()
                    .try_for_each(|package| Self::write_package_artifact(project, *package))
            })
            .context("while attempting to write package cache artifacts")
    }

    /// Drops compactable project data after package payloads have been offloaded.
    fn finish_offloading(&mut self, offloaded_packages: &[usize]) {
        if !offloaded_packages.is_empty() {
            // Offloading drops many strong `Name` handles from phase payloads. Prune the interner
            // immediately so dead weak entries and their Arc control blocks do not pin allocator
            // pages until a later rebuild happens to compact the project.
            self.project.names.shrink_to_fit();

            // File ids and paths remain resident as the source inventory. Line indexes are larger
            // and can be reconstructed from saved source text when a query needs LSP coordinates.
            self.project
                .parse
                .offload_line_indexes_for_packages(offloaded_packages);
        }
    }

    /// Writes one package artifact from currently resident phase payloads.
    fn write_package_artifact(project: &ProjectState, package: PackageSlot) -> anyhow::Result<()> {
        let artifact = Self::artifact_from_project(project, package)?;
        project
            .cache_store
            .write_artifact(&artifact)
            .with_context(|| {
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

    /// Builds the cross-phase artifact payload for one resident package.
    fn artifact_from_project(
        project: &ProjectState,
        package: PackageSlot,
    ) -> anyhow::Result<PackageCacheArtifact> {
        let header = project
            .cache_plan
            .artifact_header(package, &project.package_source_fingerprints)
            .with_context(|| {
                format!(
                    "while attempting to build package cache header for package {}",
                    package.0,
                )
            })?;
        let def_map = project.def_map.resident_package(package).with_context(|| {
            format!(
                "while attempting to fetch resident def-map package {}",
                package.0,
            )
        })?;
        let parse = project.parse.package(package.0).with_context(|| {
            format!(
                "while attempting to fetch parsed package {} for cache artifact",
                package.0,
            )
        })?;
        let semantic_ir = project
            .semantic_ir
            .resident_package(package)
            .with_context(|| {
                format!(
                    "while attempting to fetch resident semantic IR package {}",
                    package.0,
                )
            })?;
        let body_ir = project.body_ir.resident_package(package).with_context(|| {
            format!(
                "while attempting to fetch resident body IR package {}",
                package.0,
            )
        })?;

        Ok(PackageCacheArtifact::new(
            header,
            PackageCachePayload::new(
                parse.parse_snapshot().with_context(|| {
                    format!(
                        "while attempting to snapshot parse metadata for package {}",
                        package.0,
                    )
                })?,
                DefMapPackageBundle::new(def_map.clone()),
                SemanticIrPackageBundle::new(semantic_ir.clone()),
                PackageCacheBodyIrState::Built(Box::new(BodyIrPackageBundle::new(body_ir.clone()))),
            ),
        ))
    }

    /// Creates a short-lived Rayon pool for package artifact serialization.
    fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
        rayon::ThreadPoolBuilder::new()
            .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
            .build()
            .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
    }
}
