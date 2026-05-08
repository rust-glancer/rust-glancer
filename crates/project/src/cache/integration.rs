//! Project-owned package cache integration.
//!
//! Cache artifacts bundle several phase payloads together, so this module sits above the phase
//! databases. Lower crates expose package-level hooks, but they do not know where artifacts live or
//! which residency policy selected a package for offloading.

use std::sync::{Arc, OnceLock};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_body_ir::{BodyIrPackageBundle, PackageBodies};
use rg_def_map::{DefMapPackageBundle, Package as DefMapPackage, PackageSlot};
use rg_package_store::{LoadPackage, MalformedCacheError, PackageLoader, PackageStoreError};
use rg_semantic_ir::{PackageIr, SemanticIrPackageBundle};

use crate::{
    PackageResidency,
    cache::{
        PackageCacheArtifact, PackageCacheBodyIrState, PackageCachePayload, PackageCacheStore,
        WorkspaceCachePlan,
    },
    project::state::ProjectState,
};

/// Writes durable backing artifacts and offloads packages selected by the current policy.
pub(crate) fn apply_residency(project: &mut ProjectState) -> anyhow::Result<()> {
    let packages = (0..project.workspace.packages().len())
        .map(PackageSlot)
        .collect::<Vec<_>>();
    write_and_offload_packages(project, &packages)
}

/// Restores the current residency policy after a package rebuild.
///
/// Rebuilds replace the changed packages while lazily reading any dependencies they inspect. Only
/// rebuilt packages need fresh artifacts; unchanged packages can be dropped back to their
/// already-written cache entries.
pub(crate) fn restore_residency_after_rebuild(
    project: &mut ProjectState,
    rebuilt_packages: &[PackageSlot],
) -> anyhow::Result<()> {
    let package_count = project.workspace.packages().len();
    let mut rebuilt = vec![false; package_count];
    for package in rebuilt_packages {
        if package.0 < package_count {
            rebuilt[package.0] = true;
        }
    }

    let packages_to_write = rebuilt
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(package_idx, was_rebuilt)| {
            let package = PackageSlot(package_idx);
            (was_rebuilt
                && project.package_residency.package(package)
                    == Some(PackageResidency::Offloadable))
            .then_some(package)
        })
        .collect::<Vec<_>>();
    write_package_artifacts(project, &packages_to_write)?;

    let mut offloaded_packages = Vec::new();

    for package_idx in 0..package_count {
        let package = PackageSlot(package_idx);
        if project.package_residency.package(package) != Some(PackageResidency::Offloadable) {
            continue;
        }

        offload_package(project, package)?;
        offloaded_packages.push(package_idx);
    }

    finish_offloading(project, &offloaded_packages, true);
    project
        .cache_store
        .cleanup_stale_generations()
        .context("while attempting to clean stale package cache generations")?;

    Ok(())
}

fn write_and_offload_packages(
    project: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    let packages_to_offload = packages
        .iter()
        .copied()
        .filter(|package| {
            project.package_residency.package(*package) == Some(PackageResidency::Offloadable)
        })
        .collect::<Vec<_>>();
    // Cache artifacts are the durable backing store for offloadable packages. Resident packages
    // stay in memory and should not pay serialization/write cost until policy asks for it.
    write_package_artifacts(project, &packages_to_offload)?;

    let mut offloaded_packages = Vec::new();
    for package in packages_to_offload {
        offload_package(project, package)?;
        offloaded_packages.push(package.0);
    }

    finish_offloading(
        project,
        &offloaded_packages,
        packages.len() == project.parse.package_count(),
    );
    project
        .cache_store
        .cleanup_stale_generations()
        .context("while attempting to clean stale package cache generations")?;

    Ok(())
}

fn write_package_artifacts(project: &ProjectState, packages: &[PackageSlot]) -> anyhow::Result<()> {
    if packages.len() <= 1 {
        for package in packages {
            write_package_artifact(project, *package)?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-cache-write")?;

    // Artifact serialization is package-local and usually more expensive than the final state
    // mutation. Write every durable artifact first; only then can callers safely drop residents.
    thread_pool
        .install(|| {
            packages
                .par_iter()
                .try_for_each(|package| write_package_artifact(project, *package))
        })
        .context("while attempting to write package cache artifacts")
}

fn finish_offloading(
    project: &mut ProjectState,
    offloaded_packages: &[usize],
    is_full_residency_pass: bool,
) {
    if !offloaded_packages.is_empty() {
        // Offloading drops many strong `Name` handles from phase payloads. Prune the interner
        // immediately so dead weak entries and their Arc control blocks do not pin allocator pages
        // until a later rebuild happens to compact the project.
        project.names.shrink_to_fit();

        if is_full_residency_pass {
            // Parse metadata survives package offloading because it is the source map for editor
            // locations. Once the global residency plan is restored, pack stable offloaded source
            // maps into shared buffers so they do not keep many small allocations around.
            if offloaded_packages.len() == project.parse.package_count() {
                project.parse.pack_line_indexes();
            } else {
                project
                    .parse
                    .pack_line_indexes_for_packages(offloaded_packages);
            }
        }
    }
}

fn write_package_artifact(project: &ProjectState, package: PackageSlot) -> anyhow::Result<()> {
    let artifact = artifact_from_project(project, package)?;
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

fn offload_package(project: &mut ProjectState, package: PackageSlot) -> anyhow::Result<()> {
    // Only drop resident data after the full cross-phase package artifact is durable. If a future
    // implementation downgrades write errors to warnings, this invariant should remain.
    project
        .def_map
        .offload_package(package)
        .with_context(|| format!("while attempting to offload def-map package {}", package.0))?;
    project
        .semantic_ir
        .offload_package(package)
        .with_context(|| {
            format!(
                "while attempting to offload semantic IR package {}",
                package.0
            )
        })?;
    project
        .body_ir
        .offload_package(package)
        .with_context(|| format!("while attempting to offload body IR package {}", package.0))?;

    Ok(())
}

fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
        .build()
        .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
}

pub(crate) fn recover_residency_after_cache_load_failure(
    project: &mut ProjectState,
) -> anyhow::Result<()> {
    project
        .cache_store
        .invalidate_workspace_cache()
        .context("while attempting to invalidate package cache namespace")?;
    crate::project::update::rebuild_resident_from_source(project)
        .context("while attempting to rebuild resident analysis project from source")?;
    apply_residency(project).context("while attempting to reapply package cache residency")?;

    Ok(())
}

/// Loader adapters that share one package-artifact read cache.
#[derive(Clone)]
pub(crate) struct PackageReadLoaders {
    pub(crate) def_map: PackageLoader<'static, DefMapPackage>,
    pub(crate) semantic_ir: PackageLoader<'static, PackageIr>,
    pub(crate) body_ir: PackageLoader<'static, PackageBodies>,
}

pub(crate) fn package_read_loaders(project: &ProjectState) -> PackageReadLoaders {
    let bundle_loader = Arc::new(PackageBundleLoader::new(project));
    PackageReadLoaders {
        def_map: PackageLoader::new(DefMapPackageLoader {
            bundles: Arc::clone(&bundle_loader),
        }),
        semantic_ir: PackageLoader::new(SemanticIrPackageLoader {
            bundles: Arc::clone(&bundle_loader),
        }),
        body_ir: PackageLoader::new(BodyIrPackageLoader {
            bundles: bundle_loader,
        }),
    }
}

/// Shared request cache for package artifacts read by the phase-specific loaders.
#[derive(Debug)]
struct PackageBundleLoader {
    cache_plan: WorkspaceCachePlan,
    cache_store: PackageCacheStore,
    bundles: Vec<OnceLock<Arc<LoadedPackageBundle>>>,
}

impl PackageBundleLoader {
    fn new(project: &ProjectState) -> Self {
        let package_count = project.workspace.packages().len();
        Self {
            cache_plan: project.cache_plan.clone(),
            cache_store: project.cache_store.clone(),
            bundles: (0..package_count).map(|_| OnceLock::new()).collect(),
        }
    }

    fn load_bundle(&self, package: PackageSlot) -> Result<&LoadedPackageBundle, PackageStoreError> {
        let Some(cell) = self.bundles.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };

        if let Some(bundle) = cell.get() {
            return Ok(bundle.as_ref());
        }

        let bundle = Arc::new(self.load_bundle_uncached(package)?);
        let _ = cell.set(bundle);
        Ok(cell
            .get()
            .expect("package bundle cell should be initialized after successful load")
            .as_ref())
    }

    fn load_bundle_uncached(
        &self,
        package: PackageSlot,
    ) -> Result<LoadedPackageBundle, PackageStoreError> {
        let artifact = self.read_artifact(package)?;
        let payload = artifact.payload;
        let body_ir = body_ir_package_from_payload(package, payload.body_ir).map_err(|error| {
            PackageStoreError::malformed_cache(
                package,
                MalformedCacheError::InvalidPayload {
                    reason: error.to_string(),
                },
            )
        })?;

        Ok(LoadedPackageBundle {
            def_map: Arc::new(payload.def_map.into_package()),
            semantic_ir: Arc::new(payload.semantic_ir.into_package()),
            body_ir: Arc::new(body_ir),
        })
    }

    fn read_artifact(
        &self,
        package: PackageSlot,
    ) -> Result<PackageCacheArtifact, PackageStoreError> {
        let Some(header) = self.cache_plan.artifact_header(package) else {
            return Err(PackageStoreError::stale_package(
                package,
                "workspace cache plan has no package header",
            ));
        };

        match self.cache_store.read_artifact(&header) {
            Ok(Some(artifact)) => Ok(artifact),
            Ok(None) => Err(PackageStoreError::missing_package(package)),
            Err(error) => Err(error.into_package_store_error(package)),
        }
    }
}

#[derive(Debug)]
struct LoadedPackageBundle {
    def_map: Arc<DefMapPackage>,
    semantic_ir: Arc<PackageIr>,
    body_ir: Arc<PackageBodies>,
}

#[derive(Debug)]
struct DefMapPackageLoader {
    bundles: Arc<PackageBundleLoader>,
}

impl LoadPackage<DefMapPackage> for DefMapPackageLoader {
    fn load(&self, slot: PackageSlot) -> Result<Arc<DefMapPackage>, PackageStoreError> {
        Ok(Arc::clone(&self.bundles.load_bundle(slot)?.def_map))
    }
}

#[derive(Debug)]
struct SemanticIrPackageLoader {
    bundles: Arc<PackageBundleLoader>,
}

impl LoadPackage<PackageIr> for SemanticIrPackageLoader {
    fn load(&self, slot: PackageSlot) -> Result<Arc<PackageIr>, PackageStoreError> {
        Ok(Arc::clone(&self.bundles.load_bundle(slot)?.semantic_ir))
    }
}

#[derive(Debug)]
struct BodyIrPackageLoader {
    bundles: Arc<PackageBundleLoader>,
}

impl LoadPackage<PackageBodies> for BodyIrPackageLoader {
    fn load(&self, slot: PackageSlot) -> Result<Arc<PackageBodies>, PackageStoreError> {
        Ok(Arc::clone(&self.bundles.load_bundle(slot)?.body_ir))
    }
}

fn artifact_from_project(
    project: &ProjectState,
    package: PackageSlot,
) -> anyhow::Result<PackageCacheArtifact> {
    let header = project
        .cache_plan
        .artifact_header(package)
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
            DefMapPackageBundle::new(def_map.clone()),
            SemanticIrPackageBundle::new(semantic_ir.clone()),
            PackageCacheBodyIrState::Built(Box::new(BodyIrPackageBundle::new(body_ir.clone()))),
        ),
    ))
}

fn body_ir_package_from_payload(
    package: PackageSlot,
    body_ir: PackageCacheBodyIrState,
) -> anyhow::Result<PackageBodies> {
    match body_ir {
        PackageCacheBodyIrState::Built(bundle) => Ok(bundle.into_package()),
        PackageCacheBodyIrState::SkippedByPolicy => {
            anyhow::bail!(
                "package cache artifact for package {} skipped body IR payload",
                package.0,
            )
        }
    }
}
