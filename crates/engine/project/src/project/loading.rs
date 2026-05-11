//! Lazy package loading from cache artifacts.
//!
//! The phase databases work with phase-specific package stores. Cache artifacts are bundled across
//! phases, so this module adapts one artifact read into the three package loaders used by project
//! queries and rebuilds.

use std::sync::{Arc, OnceLock};

use rg_body_ir::PackageBodies;
use rg_def_map::{Package as DefMapPackage, PackageSlot};
use rg_package_store::{LoadPackage, MalformedCacheError, PackageLoader, PackageStoreError};
use rg_semantic_ir::PackageIr;

use crate::cache::{
    Fingerprint, PackageCacheArtifact, PackageCacheBodyIrState, PackageCacheStore,
    WorkspaceCachePlan,
};

use super::state::ProjectState;

/// Loader adapters that share one package-artifact read cache.
#[derive(Clone)]
pub(crate) struct PackageReadLoaders {
    pub(crate) def_map: PackageLoader<'static, DefMapPackage>,
    pub(crate) semantic_ir: PackageLoader<'static, PackageIr>,
    pub(crate) body_ir: PackageLoader<'static, PackageBodies>,
}

impl PackageReadLoaders {
    pub(crate) fn new(project: &ProjectState) -> Self {
        Self::from_cache(
            project.cache_plan.clone(),
            project.cache_store.clone(),
            project.package_source_fingerprints.clone(),
        )
    }

    pub(crate) fn from_cache(
        cache_plan: WorkspaceCachePlan,
        cache_store: PackageCacheStore,
        package_source_fingerprints: Vec<Option<Fingerprint>>,
    ) -> Self {
        let bundle_loader = Arc::new(PackageBundleLoader::new(
            cache_plan,
            cache_store,
            package_source_fingerprints,
        ));
        Self {
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
}

/// Shared request cache for package artifacts read by the phase-specific loaders.
#[derive(Debug)]
struct PackageBundleLoader {
    cache_plan: WorkspaceCachePlan,
    cache_store: PackageCacheStore,
    package_source_fingerprints: Vec<Option<Fingerprint>>,
    bundles: Vec<OnceLock<Arc<LoadedPackageBundle>>>,
}

impl PackageBundleLoader {
    fn new(
        cache_plan: WorkspaceCachePlan,
        cache_store: PackageCacheStore,
        package_source_fingerprints: Vec<Option<Fingerprint>>,
    ) -> Self {
        let package_count = package_source_fingerprints.len();
        Self {
            cache_plan,
            cache_store,
            package_source_fingerprints,
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
        let body_ir = match payload.body_ir {
            PackageCacheBodyIrState::Built(bundle) => bundle.into_package(),
            PackageCacheBodyIrState::SkippedByPolicy => {
                return Err(PackageStoreError::malformed_cache(
                    package,
                    MalformedCacheError::InvalidPayload {
                        reason: format!(
                            "package cache artifact for package {} skipped body IR payload",
                            package.0,
                        ),
                    },
                ));
            }
        };

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
        let Some(header) = self
            .cache_plan
            .artifact_header(package, &self.package_source_fingerprints)
        else {
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
