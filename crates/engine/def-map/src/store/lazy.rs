//! Request-local state for crate-granular reads from one offloaded DefMap package.
//!
//! An offloaded package keeps only its compact [`PackageDefMapsManifest`] in the project snapshot.
//! Queries use that manifest for package metadata, file routing, and dependency traversal. If a
//! query needs module scopes for one target, its [`CrateData`] is decoded into a separate cell.
//! Broad callers can still request [`PackageDefMaps`], but doing so fills every crate cell first.
//!
//! All decoded values belong to the read transaction. Dropping the transaction releases them
//! instead of turning a one-off hover into retained project memory.

use std::sync::{Arc, OnceLock};

use rg_ir_model::{CrateId, CrateRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use super::DefMapLoader;
use crate::{CrateData, PackageDefMaps, PackageDefMapsManifest, PackageSlot};

/// How one package slot participates in a DefMap read transaction.
#[derive(Debug, Clone)]
pub(super) enum PackageReadEntry<'db> {
    /// The full package is already present in the retained project snapshot.
    Resident(Arc<PackageDefMaps>),
    /// Only the package directory is retained; crate maps are loaded when requested.
    Lazy(LazyPackage<'db>),
    /// The transaction subset deliberately leaves this package inaccessible.
    Excluded,
}

/// A request-local view of one offloaded package.
///
/// The package directory and every decoded crate have independent cells. This lets exact queries
/// share work inside one transaction without making a crate resident for later requests.
#[derive(Debug, Clone)]
pub(super) struct LazyPackage<'db> {
    loader: DefMapLoader<'db>,
    loaded: OnceLock<LoadedPackage>,
}

impl<'db> LazyPackage<'db> {
    pub(super) fn new(
        loader: DefMapLoader<'db>,
        manifest: Option<Arc<PackageDefMapsManifest>>,
    ) -> Self {
        let loaded = OnceLock::new();
        if let Some(manifest) = manifest {
            let _ = loaded.set(LoadedPackage::new(manifest));
        }
        Self { loader, loaded }
    }

    /// Returns the compact package directory, loading it if startup did not retain it.
    pub(super) fn manifest(
        &self,
        package: PackageSlot,
    ) -> Result<&PackageDefMapsManifest, PackageStoreError> {
        Ok(&self.loaded(package)?.manifest)
    }

    /// Returns one exact crate map and leaves sibling crate cells untouched.
    pub(super) fn crate_data(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&CrateData>, PackageStoreError> {
        // Once a broad caller has reconstructed the package, use that single representation. Before
        // that happens, an exact read initializes only the requested crate cell.
        let loaded = self.loaded(crate_ref.package)?;
        if let Some(package) = loaded.package.get() {
            return Ok(package.crate_data(crate_ref.crate_id));
        }
        let Some(cell) = loaded.crates.get(crate_ref.crate_id.0) else {
            return Ok(None);
        };
        if cell.get().is_none() {
            let crate_data = self
                .loader
                .load_crate(crate_ref.package, crate_ref.crate_id)?;
            let _ = cell.set(crate_data);
        }
        Ok(cell.get().map(Arc::as_ref))
    }

    /// Reconstructs the broad package representation for callers that need every crate map.
    ///
    /// Exact query paths should prefer [`Self::crate_data`]. This package-wide path deliberately
    /// loads all crate shards and then validates them against the compact package directory.
    pub(super) fn package(
        &self,
        package: PackageSlot,
    ) -> Result<&PackageDefMaps, PackageStoreError> {
        let loaded = self.loaded(package)?;
        if loaded.package.get().is_none() {
            let mut crates = Vec::with_capacity(loaded.crates.len());
            for crate_idx in 0..loaded.crates.len() {
                let crate_ref = CrateRef {
                    package,
                    crate_id: CrateId(crate_idx),
                };
                let crate_data = self.crate_data(crate_ref)?.ok_or_else(|| {
                    PackageStoreError::stale_package(
                        package,
                        format!("DefMap crate {crate_idx} is absent from its manifest"),
                    )
                })?;
                crates.push(crate_data.clone());
            }
            let package_data = PackageDefMaps::from_storage_parts(&loaded.manifest, crates)
                .map(Arc::new)
                .map_err(|error| PackageStoreError::stale_package(package, format!("{error:#}")))?;
            let _ = loaded.package.set(package_data);
        }
        Ok(loaded
            .package
            .get()
            .expect("DefMap package cell should be initialized after successful load"))
    }

    /// Routes a source file to matching crate slots using only the compact package directory.
    pub(super) fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Result<Vec<CrateRef>, PackageStoreError> {
        // File ownership is part of the compact directory, so routing a source file must not decode
        // the module scopes of every target that belongs to the package.
        Ok(self
            .manifest(package)?
            .crates()
            .iter()
            .enumerate()
            .filter_map(|(crate_idx, manifest)| {
                manifest.files().contains(&file).then_some(CrateRef {
                    package,
                    crate_id: CrateId(crate_idx),
                })
            })
            .collect())
    }

    fn loaded(&self, package: PackageSlot) -> Result<&LoadedPackage, PackageStoreError> {
        // Startup may have supplied the directory already. Otherwise the first query reads only the
        // directory and uses its dense crate list to allocate the exact-read cells.
        if self.loaded.get().is_none() {
            let manifest = self.loader.load_manifest(package)?;
            let loaded = LoadedPackage::new(manifest);
            let _ = self.loaded.set(loaded);
        }
        Ok(self
            .loaded
            .get()
            .expect("DefMap package directory should be initialized after successful load"))
    }
}

/// The directory and decoded values shared by exact reads in one transaction.
#[derive(Debug, Clone)]
struct LoadedPackage {
    manifest: Arc<PackageDefMapsManifest>,
    crates: Vec<OnceLock<Arc<CrateData>>>,
    package: OnceLock<Arc<PackageDefMaps>>,
}

impl LoadedPackage {
    fn new(manifest: Arc<PackageDefMapsManifest>) -> Self {
        Self {
            crates: (0..manifest.crates().len())
                .map(|_| OnceLock::new())
                .collect(),
            manifest,
            package: OnceLock::new(),
        }
    }
}
