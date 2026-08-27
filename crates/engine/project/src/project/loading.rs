//! Lazy phase loading from sectioned package cache artifacts.
//!
//! One request shares an artifact revision across phase-specific package stores. DefMap and
//! Semantic IR load crate shards, while Body IR loads source-file shards.
//!
//! All phase loaders in one request read the same immutable artifact revision, so callers do not
//! have to coordinate revisions themselves.

use std::{
    fmt,
    sync::{Arc, OnceLock},
};

use rg_body_ir::{BodyFileShard, BodyIrLoader, CrateBodies, LoadBodyIr, PackageBodiesManifest};
use rg_def_map::{CrateData, DefMapLoader, LoadDefMap, PackageDefMapsManifest, PackageSlot};
use rg_ir_model::CrateId;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_semantic_ir::{
    ItemLookupIndex, ItemStore, LoadSemanticIr, PackageIrManifest, SemanticIrLoader,
};

use crate::cache::{Fingerprint, PackageArtifactReader, PackageCacheStore, WorkspaceCachePlan};

use super::state::ProjectState;

/// Phase-specific loaders backed by one request-local set of artifact revisions.
///
/// DefMap, Semantic IR, and Body IR expose different storage units, but all three adapters resolve a
/// package slot through the same [`PackageArtifactReader`]. Decoded values belong to their phase read
/// transactions; only the open reader is shared for the duration of this loader set.
#[derive(Clone)]
pub(crate) struct PackageReadLoaders {
    pub(crate) def_map: DefMapLoader<'static>,
    pub(crate) semantic_ir: SemanticIrLoader<'static>,
    pub(crate) body_ir: BodyIrLoader<'static>,
    artifacts: Arc<PackageArtifactReaders>,
}

impl fmt::Debug for PackageReadLoaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PackageReadLoaders")
            .finish_non_exhaustive()
    }
}

impl PackageReadLoaders {
    pub(crate) fn new(project: &ProjectState) -> Self {
        Self::from_cache(
            project.cache_plan.clone(),
            project.cache_store.clone(),
            project.package_source_fingerprints.clone(),
        )
    }

    /// Creates cache readers for dependencies while excluding every package rebuilt from source.
    ///
    /// A dirty package's final fingerprint is unknown until macro source-file discovery settles, so
    /// allowing its old artifact into the build would mix two source generations.
    pub(crate) fn for_package_rebuild(
        project: &ProjectState,
        source_packages: &[PackageSlot],
    ) -> Self {
        Self::from_cache_excluding(
            project.cache_plan.clone(),
            project.cache_store.clone(),
            project.package_source_fingerprints.clone(),
            source_packages,
        )
    }

    /// Clears selected fingerprints before constructing artifact readers.
    ///
    /// Unselected packages keep their already validated fingerprints and remain available for lazy
    /// dependency reads. A selected slot has no usable artifact identity until its source build has
    /// produced the final file table and fingerprint.
    pub(crate) fn from_cache_excluding(
        cache_plan: WorkspaceCachePlan,
        cache_store: PackageCacheStore,
        mut package_source_fingerprints: Vec<Option<Fingerprint>>,
        source_packages: &[PackageSlot],
    ) -> Self {
        for package in source_packages {
            if let Some(fingerprint) = package_source_fingerprints.get_mut(package.0) {
                *fingerprint = None;
            }
        }
        debug_assert!(source_packages.iter().all(|package| {
            package_source_fingerprints
                .get(package.0)
                .is_some_and(Option::is_none)
        }));

        Self::from_cache(cache_plan, cache_store, package_source_fingerprints)
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
            def_map: DefMapLoader::new(DefMapPackageLoader {
                artifacts: Arc::clone(&artifacts),
            }),
            semantic_ir: SemanticIrLoader::new(SemanticIrPackageLoader {
                artifacts: Arc::clone(&artifacts),
            }),
            body_ir: BodyIrLoader::new(BodyIrPackageLoader {
                artifacts: Arc::clone(&artifacts),
            }),
            artifacts,
        }
    }

    /// Restores a manifest-only Body IR package shape for an exact target rebuild.
    ///
    /// The builder needs aligned crate slots so it can replace the selected target. Sibling slots
    /// retain only their file routing and coverage; their body shards remain in the artifact.
    pub(crate) fn load_body_ir_package_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<rg_body_ir::PackageBodies, PackageStoreError> {
        let reader = self.artifacts.reader(package)?;
        let body_manifest = reader
            .read_body_ir_manifest()
            .map_err(|error| error.into_package_store_error(package))?;
        Ok(rg_body_ir::PackageBodies::from_cached_manifest(
            &body_manifest,
        ))
    }
}

/// Shared request cache for package artifact revisions.
///
/// The cache lives behind `PackageReadLoaders`, so several read transactions can share it without
/// making decoded dependencies permanent project state. Operations drop their loader set on
/// return.
#[derive(Debug)]
struct PackageArtifactReaders {
    cache_plan: WorkspaceCachePlan,
    cache_store: PackageCacheStore,
    package_source_fingerprints: Vec<Option<Fingerprint>>,
    packages: Vec<OnceLock<PackageArtifactReader>>,
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
            packages: (0..package_count).map(|_| OnceLock::new()).collect(),
        }
    }

    /// Opens a package artifact once and shares that pinned revision across all phase adapters.
    ///
    /// Failed opens are not cached, so a storage error is returned with its package context rather
    /// than leaving a permanently initialized error sentinel in the request.
    fn reader(&self, package: PackageSlot) -> Result<&PackageArtifactReader, PackageStoreError> {
        let cell = self
            .packages
            .get(package.0)
            .ok_or(PackageStoreError::MissingSlot { slot: package })?;

        if let Some(reader) = cell.get() {
            return Ok(reader);
        }

        let reader = self.open_reader(package)?;
        let _ = cell.set(reader);
        Ok(cell
            .get()
            .expect("package artifact reader cell should be initialized after successful open"))
    }

    /// Reconstructs the expected artifact header from the cache plan before opening the file.
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

impl LoadDefMap for DefMapPackageLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageDefMapsManifest>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_def_map_manifest()
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_crate(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<CrateData>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_def_map_crate(crate_id)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }
}

#[derive(Debug)]
struct SemanticIrPackageLoader {
    artifacts: Arc<PackageArtifactReaders>,
}

impl LoadSemanticIr for SemanticIrPackageLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageIrManifest>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_semantic_ir_manifest()
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_items(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemStore>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_semantic_ir_items(crate_id)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
    }

    fn load_lookup_index(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        self.artifacts
            .reader(package)?
            .read_semantic_ir_lookup_index(crate_id)
            .map(Arc::new)
            .map_err(|error| error.into_package_store_error(package))
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
