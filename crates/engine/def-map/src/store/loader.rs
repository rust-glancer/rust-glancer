//! DefMap storage contract used when a package is not resident.
//!
//! The manifest contains package metadata and file-to-crate routing. Crate payloads remain separate
//! so an interactive query can load one semantic interpretation without decoding sibling targets.

use std::sync::Arc;

use rg_ir_model::CrateId;
use rg_package_store::PackageStoreError;

use crate::{CrateData, PackageDefMapsManifest, PackageSlot};

/// Loads the independently stored parts of an offloaded DefMap package.
///
/// The manifest is the small routing directory. [`Self::load_crate`] reads the module scopes for
/// exactly one Cargo target, so implementations must not decode sibling crates as a side effect.
pub trait LoadDefMap: std::fmt::Debug + Send + Sync {
    /// Loads package metadata and the routing entry for every crate slot.
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageDefMapsManifest>, PackageStoreError>;

    /// Loads the complete DefMap data for one crate slot.
    fn load_crate(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<CrateData>, PackageStoreError>;
}

/// Cloneable, lifetime-aware handle to an offloaded DefMap reader.
///
/// Read transactions clone this handle into their lazy package entries. The trait object may borrow
/// project-owned cache state, while decoded values remain owned by the transaction.
pub struct DefMapLoader<'db> {
    loader: Arc<dyn LoadDefMap + Send + Sync + 'db>,
}

impl<'db> DefMapLoader<'db> {
    pub fn new(loader: impl LoadDefMap + 'db) -> Self {
        Self {
            loader: Arc::new(loader),
        }
    }

    pub(super) fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageDefMapsManifest>, PackageStoreError> {
        self.loader.load_manifest(package)
    }

    pub(super) fn load_crate(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<CrateData>, PackageStoreError> {
        self.loader.load_crate(package, crate_id)
    }
}

impl DefMapLoader<'static> {
    /// Creates a loader that treats any offloaded read as a violated caller invariant.
    ///
    /// Build and test paths that require all packages to be resident use this form. The supplied
    /// context is included in the panic so an unexpected lazy read identifies that boundary.
    pub fn resident_only(context: &'static str) -> Self {
        Self::new(ResidentOnlyDefMapLoader { context })
    }
}

impl Clone for DefMapLoader<'_> {
    fn clone(&self) -> Self {
        Self {
            loader: Arc::clone(&self.loader),
        }
    }
}

impl std::fmt::Debug for DefMapLoader<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.loader.fmt(formatter)
    }
}

#[derive(Debug)]
struct ResidentOnlyDefMapLoader {
    context: &'static str,
}

impl LoadDefMap for ResidentOnlyDefMapLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageDefMapsManifest>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
    }

    fn load_crate(
        &self,
        package: PackageSlot,
        _crate_id: CrateId,
    ) -> Result<Arc<CrateData>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
    }
}
