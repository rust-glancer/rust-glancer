//! Semantic IR storage contract used when a package is not resident.
//!
//! The manifest discovers dense crate slots. Each crate then exposes declarations and its lookup
//! index as separate reads, matching the nested on-disk framing and keeping ordinary item queries
//! independent from visibility-wide candidate lookup.

use std::sync::Arc;

use rg_def_map::PackageSlot;
use rg_ir_model::CrateId;
use rg_package_store::PackageStoreError;

use crate::{ItemLookupIndex, ItemStore, PackageIrManifest};

/// Loads the independently stored parts of an offloaded Semantic IR package.
///
/// Declarations and lookup indexes are separate because common queries need only one of them.
/// Implementations should preserve that boundary rather than decoding the complete crate payload.
pub trait LoadSemanticIr: std::fmt::Debug + Send + Sync {
    /// Loads the crate count used to address the package's dense crate slots.
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageIrManifest>, PackageStoreError>;

    /// Loads declarations for one crate without its visibility lookup index.
    fn load_items(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemStore>, PackageStoreError>;

    /// Loads the visibility lookup index for one crate without its declarations.
    fn load_lookup_index(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError>;
}

/// Cloneable, lifetime-aware handle to an offloaded Semantic IR reader.
///
/// Read transactions clone this handle into their lazy package entries. The trait object may borrow
/// project-owned cache state, while decoded values remain owned by the transaction.
pub struct SemanticIrLoader<'db> {
    loader: Arc<dyn LoadSemanticIr + Send + Sync + 'db>,
}

impl<'db> SemanticIrLoader<'db> {
    pub fn new(loader: impl LoadSemanticIr + 'db) -> Self {
        Self {
            loader: Arc::new(loader),
        }
    }

    pub(super) fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageIrManifest>, PackageStoreError> {
        self.loader.load_manifest(package)
    }

    pub(super) fn load_items(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemStore>, PackageStoreError> {
        self.loader.load_items(package, crate_id)
    }

    pub(super) fn load_lookup_index(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        self.loader.load_lookup_index(package, crate_id)
    }
}

impl SemanticIrLoader<'static> {
    /// Creates a loader that treats any offloaded read as a violated caller invariant.
    ///
    /// Build and test paths that require all packages to be resident use this form. The supplied
    /// context is included in the panic so an unexpected lazy read identifies that boundary.
    pub fn resident_only(context: &'static str) -> Self {
        Self::new(ResidentOnlySemanticIrLoader { context })
    }
}

impl Clone for SemanticIrLoader<'_> {
    fn clone(&self) -> Self {
        Self {
            loader: Arc::clone(&self.loader),
        }
    }
}

impl std::fmt::Debug for SemanticIrLoader<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.loader.fmt(formatter)
    }
}

#[derive(Debug)]
struct ResidentOnlySemanticIrLoader {
    context: &'static str,
}

impl LoadSemanticIr for ResidentOnlySemanticIrLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageIrManifest>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
    }

    fn load_items(
        &self,
        package: PackageSlot,
        _crate_id: CrateId,
    ) -> Result<Arc<ItemStore>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
    }

    fn load_lookup_index(
        &self,
        package: PackageSlot,
        _crate_id: CrateId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
    }
}
