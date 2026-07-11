//! Request-local state for an offloaded Body IR package.
//!
//! `PackageStore` already tells the transaction whether the complete [`PackageBodies`] value is
//! resident. This module handles the other case. It starts with no decoded Body IR and loads the
//! smallest storage unit that can answer each query:
//!
//! ```text
//! first Body IR query
//!     -> package manifest
//!     -> target index, one file shard, or the complete target
//! ```
//!
//! For example, scanning `src/foo.rs` loads the manifest and the shard for `foo.rs`. Asking for all
//! bodies in the target loads every file shard. Asking for [`TargetBodies`] explicitly loads the
//! complete target representation instead.
//!
//! The loaded values live only for this read transaction. `OnceLock` lets methods return ordinary
//! borrowed references without promoting decoded shards into the retained project snapshot. A
//! failed load leaves its cell empty, so a later call can try again. If the complete target is
//! loaded, later queries read from it instead of loading another copy of the same body data.

use std::sync::{Arc, OnceLock};

use rg_def_map::PackageSlot;
use rg_ir_model::{BodyRef, TargetRef};
use rg_ir_storage::{BodyLocalItems, ItemLookupIndex};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, TargetId};

use super::BodyIrLoader;
use crate::{BodyFileShard, PackageBodies, PackageBodiesManifest, ResolvedBodyData, TargetBodies};

/// One package slot as seen by this Body IR read transaction.
///
/// The shape matches the package store's residency states, but the lazy case is Body-specific: it
/// can hold a manifest and individual file shards without constructing `PackageBodies`.
#[derive(Debug, Clone)]
pub(super) enum PackageReadEntry<'db> {
    /// The retained snapshot already owns the complete package.
    Resident(Arc<PackageBodies>),
    /// The package is present in the cache and can be decoded in smaller pieces.
    Lazy(LazyPackage<'db>),
    /// The transaction's package subset intentionally hides this slot.
    Excluded,
}

/// Lazily decoded pieces of one offloaded package.
///
/// Loading begins with `loaded`, which contains the manifest and creates empty cells for every
/// target index and file shard described by it. Those cells are then filled independently as query
/// methods need them.
#[derive(Debug, Clone)]
pub(super) struct LazyPackage<'db> {
    loader: BodyIrLoader<'db>,
    loaded: OnceLock<LoadedPackage>,
}

impl<'db> LazyPackage<'db> {
    pub(super) fn new(loader: BodyIrLoader<'db>) -> Self {
        Self {
            loader,
            loaded: OnceLock::new(),
        }
    }

    /// Load the complete target representation.
    ///
    /// This is the expensive path used by callers that genuinely need `TargetBodies`. File-local
    /// access goes through `bodies`, `body_data`, or `body_local_items` and normally loads less.
    pub(super) fn target(
        &self,
        target: TargetRef,
    ) -> Result<Option<&TargetBodies>, PackageStoreError> {
        let Some(loaded_target) = self.loaded(target.package)?.target(target.target) else {
            return Ok(None);
        };
        if loaded_target.target.get().is_none() {
            let target_bodies = self.loader.load_target(target.package, target.target)?;
            let _ = loaded_target.target.set(target_bodies);
        }
        Ok(loaded_target.target.get().map(Arc::as_ref))
    }

    /// Return the target-global item index without loading its body shards.
    ///
    /// A complete target already contains the same index, so prefer it when another query loaded
    /// the target first. Otherwise the index remains an independent cache unit.
    pub(super) fn semantic_index(
        &self,
        target: TargetRef,
    ) -> Result<Option<&ItemLookupIndex>, PackageStoreError> {
        let Some(loaded_target) = self.loaded(target.package)?.target(target.target) else {
            return Ok(None);
        };
        if let Some(target_bodies) = loaded_target.target.get() {
            return Ok(Some(target_bodies.semantic_index()));
        }
        if loaded_target.semantic_index.get().is_none() {
            let index = self
                .loader
                .load_semantic_index(target.package, target.target)?;
            let _ = loaded_target.semantic_index.set(index);
        }
        Ok(loaded_target.semantic_index.get().map(Arc::as_ref))
    }

    /// Enumerate one file's bodies, or all target bodies when `file` is absent.
    ///
    /// A resident complete target can be filtered directly. For a still-sharded target, the file
    /// argument decides whether this visits one shard or every shard from the manifest.
    pub(super) fn bodies(
        &self,
        target: TargetRef,
        file: Option<FileId>,
    ) -> Result<Vec<(BodyRef, &ResolvedBodyData)>, PackageStoreError> {
        let Some(loaded_target) = self.loaded(target.package)?.target(target.target) else {
            return Ok(Vec::new());
        };
        if let Some(target_bodies) = loaded_target.target.get() {
            return Ok(target_bodies
                .bodies
                .iter_with_ids()
                .filter(|(_, body)| file.is_none_or(|file| body.source().file_id == file))
                .map(|(body, data)| (BodyRef { target, body }, data))
                .collect());
        }

        let mut bodies = Vec::new();
        for &(shard_file, _) in &loaded_target.shards {
            if file.is_some_and(|file| file != shard_file) {
                continue;
            }
            let shard = self.file_shard(target, shard_file)?;
            bodies.extend(shard.entries().iter().map(|entry| {
                (
                    BodyRef {
                        target,
                        body: entry.body(),
                    },
                    entry.data(),
                )
            }));
        }
        Ok(bodies)
    }

    /// Find one body by using the manifest to select its source-file shard.
    ///
    /// Looking up one body does not scan or decode other file shards.
    pub(super) fn body_data(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&ResolvedBodyData>, PackageStoreError> {
        let Some((loaded_target, file)) = self.body_location(body_ref)? else {
            return Ok(None);
        };
        if let Some(target_bodies) = loaded_target.target.get() {
            return Ok(target_bodies.body(body_ref.body));
        }
        Ok(self.file_shard(body_ref.target, file)?.body(body_ref.body))
    }

    /// Find the body-local DefMap and item store paired with one body.
    ///
    /// Body data and body-local items are stored in the same file shard, so this follows the same
    /// manifest lookup as `body_data`.
    pub(super) fn body_local_items(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&BodyLocalItems>, PackageStoreError> {
        let Some((loaded_target, file)) = self.body_location(body_ref)? else {
            return Ok(None);
        };
        if let Some(target_bodies) = loaded_target.target.get() {
            return Ok(target_bodies.body_local_items(body_ref.body));
        }
        Ok(self
            .file_shard(body_ref.target, file)?
            .body_local_items(body_ref.body))
    }

    /// Resolve a stable body id to the target state and file recorded by the manifest.
    fn body_location(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<(&LoadedTarget, FileId)>, PackageStoreError> {
        let loaded = self.loaded(body_ref.target.package)?;
        let Some(target_manifest) = loaded.manifest.target(body_ref.target.target) else {
            return Ok(None);
        };
        let Some(file) = target_manifest.body_file(body_ref.body) else {
            return Ok(None);
        };
        Ok(loaded
            .target(body_ref.target.target)
            .map(|target| (target, file)))
    }

    /// Load one declared file shard and keep it alive for the rest of the transaction.
    ///
    /// A missing target or file means the loader and manifest disagree. Treat that as a stale
    /// package rather than quietly returning no bodies from a malformed cache revision.
    fn file_shard(
        &self,
        target: TargetRef,
        file: FileId,
    ) -> Result<&BodyFileShard, PackageStoreError> {
        let loaded = self.loaded(target.package)?;
        let Some(loaded_target) = loaded.target(target.target) else {
            return Err(PackageStoreError::stale_package(
                target.package,
                format!(
                    "Body IR target {:?} is absent from its manifest",
                    target.target
                ),
            ));
        };
        let Some((_, shard)) = loaded_target
            .shards
            .iter()
            .find(|(shard_file, _)| *shard_file == file)
        else {
            return Err(PackageStoreError::stale_package(
                target.package,
                format!("Body IR file {:?} is absent from its target manifest", file),
            ));
        };
        if shard.get().is_none() {
            let loaded_shard = self
                .loader
                .load_file_shard(target.package, target.target, file)?;
            let _ = shard.set(loaded_shard);
        }
        Ok(shard
            .get()
            .expect("Body IR shard cell should be initialized after successful load"))
    }

    /// Load the package manifest and use it to allocate the request-local target cells.
    fn loaded(&self, package: PackageSlot) -> Result<&LoadedPackage, PackageStoreError> {
        if self.loaded.get().is_none() {
            let manifest = self.loader.load_manifest(package)?;
            let loaded = LoadedPackage::new(manifest);
            let _ = self.loaded.set(loaded);
        }
        Ok(self
            .loaded
            .get()
            .expect("Body IR package cell should be initialized after successful load"))
    }
}

/// Manifest plus empty-or-loaded state for every target declared by it.
#[derive(Debug, Clone)]
struct LoadedPackage {
    manifest: Arc<PackageBodiesManifest>,
    targets: Vec<LoadedTarget>,
}

impl LoadedPackage {
    fn new(manifest: Arc<PackageBodiesManifest>) -> Self {
        let targets = manifest
            .targets()
            .iter()
            .map(|target| LoadedTarget {
                semantic_index: OnceLock::new(),
                shards: target
                    .files()
                    .iter()
                    .copied()
                    .map(|file| (file, OnceLock::new()))
                    .collect(),
                target: OnceLock::new(),
            })
            .collect();
        Self { manifest, targets }
    }

    fn target(&self, target: TargetId) -> Option<&LoadedTarget> {
        self.targets.get(target.0)
    }
}

/// Independently loadable pieces of one target.
///
/// `target` is the complete fallback representation. Once it is present, query methods prefer it
/// over `semantic_index` and `shards`, but already-loaded smaller pieces remain harmless and keep
/// any references returned earlier by the transaction valid.
#[derive(Debug, Clone)]
struct LoadedTarget {
    semantic_index: OnceLock<Arc<ItemLookupIndex>>,
    shards: Vec<(FileId, OnceLock<Arc<BodyFileShard>>)>,
    target: OnceLock<Arc<TargetBodies>>,
}
