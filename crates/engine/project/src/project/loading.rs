//! Lazy phase loading from sectioned package cache artifacts.
//!
//! One request shares an artifact revision and its decoded declaration payloads across
//! phase-specific package stores. Body IR sections remain independently lazy, but DefMap and
//! Semantic IR packages are decoded at most once while this loader set is alive.

use std::sync::{Arc, OnceLock};

use rg_body_ir::{BodyFileShard, BodyIrLoader, CrateBodies, LoadBodyIr, PackageBodiesManifest};
use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_def_map::PackageSlot;
use rg_ir_model::CrateId;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::FileId;
use rg_semantic_ir::ItemLookupIndex;
use rg_semantic_ir::PackageIr;

use crate::cache::{Fingerprint, PackageArtifactReader, PackageCacheStore, WorkspaceCachePlan};

use super::state::ProjectState;

/// Loader adapters that share one package-artifact read cache.
#[derive(Clone)]
pub(crate) struct PackageReadLoaders {
    pub(crate) def_map: PackageLoader<'static, DefMapPackage>,
    pub(crate) semantic_ir: PackageLoader<'static, PackageIr>,
    pub(crate) body_ir: BodyIrLoader<'static>,
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
        let artifacts = Arc::new(PackageArtifactReaders::new(
            cache_plan,
            cache_store,
            package_source_fingerprints,
        ));
        Self {
            def_map: PackageLoader::new(DefMapPackageLoader {
                artifacts: Arc::clone(&artifacts),
            }),
            semantic_ir: PackageLoader::new(SemanticIrPackageLoader {
                artifacts: Arc::clone(&artifacts),
            }),
            body_ir: BodyIrLoader::new(BodyIrPackageLoader { artifacts }),
        }
    }
}

/// Shared request cache for package artifact revisions and decoded declaration payloads.
///
/// The cache belongs to `PackageReadLoaders`, not `ProjectState`. Rebuilds and queries can create
/// several independent read transactions without decoding the same package for each one, while
/// dropping the outer operation still releases every offloaded payload it loaded.
#[derive(Debug)]
struct PackageArtifactReaders {
    cache_plan: WorkspaceCachePlan,
    cache_store: PackageCacheStore,
    package_source_fingerprints: Vec<Option<Fingerprint>>,
    readers: Vec<OnceLock<PackageArtifactReader>>,
    def_maps: Vec<OnceLock<Arc<DefMapPackage>>>,
    semantic_irs: Vec<OnceLock<Arc<PackageIr>>>,
}

impl PackageArtifactReaders {
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
            readers: (0..package_count).map(|_| OnceLock::new()).collect(),
            def_maps: (0..package_count).map(|_| OnceLock::new()).collect(),
            semantic_irs: (0..package_count).map(|_| OnceLock::new()).collect(),
        }
    }

    fn def_map(&self, package: PackageSlot) -> Result<Arc<DefMapPackage>, PackageStoreError> {
        let Some(cell) = self.def_maps.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };
        if let Some(def_map) = cell.get() {
            return Ok(Arc::clone(def_map));
        }

        // Failed decodes are deliberately not cached: a later load keeps the package-store
        // transaction's existing retry behavior. Concurrent first loads may duplicate work, but
        // every successful caller converges on the one value retained by the cell.
        let def_map = self
            .reader(package)?
            .read_def_map()
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))?;
        let _ = cell.set(def_map);
        Ok(Arc::clone(cell.get().expect(
            "decoded def-map cell should be initialized after successful load",
        )))
    }

    fn semantic_ir(&self, package: PackageSlot) -> Result<Arc<PackageIr>, PackageStoreError> {
        let Some(cell) = self.semantic_irs.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };
        if let Some(semantic_ir) = cell.get() {
            return Ok(Arc::clone(semantic_ir));
        }

        let semantic_ir = self
            .reader(package)?
            .read_semantic_ir()
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))?;
        let _ = cell.set(semantic_ir);
        Ok(Arc::clone(cell.get().expect(
            "decoded semantic-IR cell should be initialized after successful load",
        )))
    }

    fn reader(&self, package: PackageSlot) -> Result<&PackageArtifactReader, PackageStoreError> {
        let Some(cell) = self.readers.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };

        if let Some(reader) = cell.get() {
            return Ok(reader);
        }

        let reader = self.open_reader(package)?;
        let _ = cell.set(reader);
        Ok(cell
            .get()
            .expect("package artifact reader cell should be initialized after successful open"))
    }

    fn open_reader(
        &self,
        package: PackageSlot,
    ) -> Result<PackageArtifactReader, PackageStoreError> {
        let Some(header) = self
            .cache_plan
            .artifact_header(package, &self.package_source_fingerprints)
        else {
            return Err(PackageStoreError::stale_package(
                package,
                "workspace cache plan has no package header",
            ));
        };

        match self.cache_store.open_artifact(&header) {
            Ok(Some(reader)) => Ok(reader),
            Ok(None) => Err(PackageStoreError::missing_package(package)),
            Err(error) => Err(error.into_package_store_error(package)),
        }
    }
}

#[derive(Debug)]
struct DefMapPackageLoader {
    artifacts: Arc<PackageArtifactReaders>,
}

impl LoadPackage<DefMapPackage> for DefMapPackageLoader {
    fn load(&self, slot: PackageSlot) -> Result<Arc<DefMapPackage>, PackageStoreError> {
        self.artifacts.def_map(slot)
    }
}

#[derive(Debug)]
struct SemanticIrPackageLoader {
    artifacts: Arc<PackageArtifactReaders>,
}

impl LoadPackage<PackageIr> for SemanticIrPackageLoader {
    fn load(&self, slot: PackageSlot) -> Result<Arc<PackageIr>, PackageStoreError> {
        self.artifacts.semantic_ir(slot)
    }
}

#[derive(Debug)]
struct BodyIrPackageLoader {
    artifacts: Arc<PackageArtifactReaders>,
}

impl LoadBodyIr for BodyIrPackageLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageBodiesManifest>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_body_ir_manifest()
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_semantic_index(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_body_semantic_index(crate_id)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_file_shard(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
        file: FileId,
    ) -> Result<Arc<BodyFileShard>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_body_file_shard(crate_id, file)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_crate(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<CrateBodies>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_body_crate(crate_id)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }
}
