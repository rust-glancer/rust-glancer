//! Body IR storage contract used when a package is not resident.
//!
//! The trait speaks in Body IR units rather than cache files. This keeps the project layer free to
//! choose its artifact format, while the transaction can explicitly request the manifest, one
//! target-global index, one source-file shard, or a complete target.

use std::sync::Arc;

use rg_def_map::PackageSlot;
use rg_ir_storage::ItemLookupIndex;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, TargetId};

use crate::{BodyFileShard, PackageBodiesManifest, TargetBodies};

/// Loads the storage units of one offloaded Body IR package.
pub trait LoadBodyIr: std::fmt::Debug + Send + Sync {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageBodiesManifest>, PackageStoreError>;

    fn load_semantic_index(
        &self,
        package: PackageSlot,
        target: TargetId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError>;

    fn load_file_shard(
        &self,
        package: PackageSlot,
        target: TargetId,
        file: FileId,
    ) -> Result<Arc<BodyFileShard>, PackageStoreError>;

    fn load_target(
        &self,
        package: PackageSlot,
        target: TargetId,
    ) -> Result<Arc<TargetBodies>, PackageStoreError>;
}

/// Shared loader used by Body IR read transactions.
pub struct BodyIrLoader<'db> {
    loader: Arc<dyn LoadBodyIr + Send + Sync + 'db>,
}

impl<'db> BodyIrLoader<'db> {
    pub fn new(loader: impl LoadBodyIr + 'db) -> Self {
        Self {
            loader: Arc::new(loader),
        }
    }

    pub(super) fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageBodiesManifest>, PackageStoreError> {
        self.loader.load_manifest(package)
    }

    pub(super) fn load_semantic_index(
        &self,
        package: PackageSlot,
        target: TargetId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        self.loader.load_semantic_index(package, target)
    }

    pub(super) fn load_file_shard(
        &self,
        package: PackageSlot,
        target: TargetId,
        file: FileId,
    ) -> Result<Arc<BodyFileShard>, PackageStoreError> {
        self.loader.load_file_shard(package, target, file)
    }

    pub(super) fn load_target(
        &self,
        package: PackageSlot,
        target: TargetId,
    ) -> Result<Arc<TargetBodies>, PackageStoreError> {
        self.loader.load_target(package, target)
    }
}

impl BodyIrLoader<'static> {
    pub fn resident_only(context: &'static str) -> Self {
        Self::new(ResidentOnlyBodyIrLoader { context })
    }
}

impl Clone for BodyIrLoader<'_> {
    fn clone(&self) -> Self {
        Self {
            loader: Arc::clone(&self.loader),
        }
    }
}

impl std::fmt::Debug for BodyIrLoader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.loader.fmt(f)
    }
}

#[derive(Debug)]
struct ResidentOnlyBodyIrLoader {
    context: &'static str,
}

impl LoadBodyIr for ResidentOnlyBodyIrLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageBodiesManifest>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0
        )
    }

    fn load_semantic_index(
        &self,
        package: PackageSlot,
        _target: TargetId,
    ) -> Result<Arc<ItemLookupIndex>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0
        )
    }

    fn load_file_shard(
        &self,
        package: PackageSlot,
        _target: TargetId,
        _file: FileId,
    ) -> Result<Arc<BodyFileShard>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0
        )
    }

    fn load_target(
        &self,
        package: PackageSlot,
        _target: TargetId,
    ) -> Result<Arc<TargetBodies>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0
        )
    }
}
