//! Request-local state for an offloaded Body IR package.
//!
//! `PackageStore` already tells the transaction whether the complete [`PackageBodies`] value is
//! resident. This module handles the other case. It starts with no decoded Body IR and loads the
//! smallest storage unit that can answer each query:
//!
//! ```text
//! first Body IR query
//!     -> package manifest
//!     -> item lookup index, one file shard, or the complete crate
//! ```
//!
//! For example, scanning `src/foo.rs` loads the manifest and the shard for `foo.rs`. Asking for all
//! bodies in the crate loads every file shard. Asking for [`CrateBodies`] explicitly loads the
//! complete crate representation instead.
//!
//! The loaded values live only for this read transaction. `OnceLock` lets methods return ordinary
//! borrowed references without promoting decoded shards into the retained project snapshot. A
//! failed load leaves its cell empty, so a later call can try again. If the complete crate is
//! loaded, later queries read from it instead of loading another copy of the same body data.

use std::sync::{Arc, OnceLock};

use rg_def_map::PackageSlot;
use rg_ir_model::{BodyRef, CrateId, CrateRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_semantic_ir::ItemLookupIndex;

use super::BodyIrLoader;
use crate::{
    BodyFileShard, BodyLocalItems, BodyView, CrateBodies, PackageBodies, PackageBodiesManifest,
};

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
/// item lookup index and file shard described by it. Those cells are then filled independently as
/// query methods need them.
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

    /// Return the saved dense body count without decoding any body shard.
    pub(super) fn body_count(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<usize>, PackageStoreError> {
        Ok(self
            .loaded(crate_ref.package)?
            .manifest
            .crate_manifest(crate_ref.crate_id)
            .map(|manifest| manifest.body_count()))
    }

    /// Load the complete crate representation.
    ///
    /// This is the expensive path used by callers that genuinely need `CrateBodies`. File-local
    /// access goes through `bodies`, `body`, or `body_local_items` and normally loads less.
    pub(super) fn crate_bodies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&CrateBodies>, PackageStoreError> {
        let Some(loaded_crate) = self
            .loaded(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
        else {
            return Ok(None);
        };
        if loaded_crate.bodies.get().is_none() {
            let crate_bodies = self
                .loader
                .load_crate(crate_ref.package, crate_ref.crate_id)?;
            let _ = loaded_crate.bodies.set(crate_bodies);
        }
        Ok(loaded_crate.bodies.get().map(Arc::as_ref))
    }

    /// Return the crate-global item index without loading its body shards.
    ///
    /// A complete crate already contains the same index, so prefer it when another query loaded
    /// the crate first. Otherwise the index remains an independent cache unit. Crates with
    /// `Missing` or `SkippedByPolicy` coverage have no published index even though their cache
    /// payload contains an empty placeholder.
    pub(super) fn item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&ItemLookupIndex>, PackageStoreError> {
        let loaded = self.loaded(crate_ref.package)?;
        let Some(crate_manifest) = loaded.manifest.crate_manifest(crate_ref.crate_id) else {
            return Ok(None);
        };
        if !crate_manifest.coverage().is_materialized() {
            return Ok(None);
        }
        let Some(loaded_crate) = loaded.crate_data(crate_ref.crate_id) else {
            return Ok(None);
        };
        if let Some(crate_bodies) = loaded_crate.bodies.get() {
            return Ok(Some(crate_bodies.item_lookup_index()));
        }
        if loaded_crate.item_lookup_index.get().is_none() {
            let index = self
                .loader
                .load_item_lookup_index(crate_ref.package, crate_ref.crate_id)?;
            let _ = loaded_crate.item_lookup_index.set(index);
        }
        Ok(loaded_crate.item_lookup_index.get().map(Arc::as_ref))
    }

    /// Enumerate one file's bodies, or all crate bodies when `file` is absent.
    ///
    /// A resident complete crate can be filtered directly. For a still-sharded crate, the file
    /// argument decides whether this visits one shard or every shard from the manifest.
    pub(super) fn bodies(
        &self,
        crate_ref: CrateRef,
        file: Option<FileId>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, PackageStoreError> {
        let Some(loaded_crate) = self
            .loaded(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
        else {
            return Ok(Vec::new());
        };
        if let Some(crate_bodies) = loaded_crate.bodies.get() {
            return Ok(crate_bodies
                .body_views()
                .filter(|(_, body)| file.is_none_or(|file| body.source().file_id == file))
                .map(|(body, view)| (BodyRef { crate_ref, body }, view))
                .collect());
        }

        let mut bodies = Vec::new();
        for &(shard_file, _) in &loaded_crate.shards {
            if file.is_some_and(|file| file != shard_file) {
                continue;
            }
            let shard = self.file_shard(crate_ref, shard_file)?;
            bodies.extend(shard.entries().iter().map(|entry| {
                (
                    BodyRef {
                        crate_ref,
                        body: entry.body(),
                    },
                    entry.view(),
                )
            }));
        }
        Ok(bodies)
    }

    /// Find one body by using the manifest to select its source-file shard.
    ///
    /// Looking up one body does not scan or decode other file shards.
    pub(super) fn body(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<BodyView<'_>>, PackageStoreError> {
        let Some((loaded_crate, file)) = self.body_location(body_ref)? else {
            return Ok(None);
        };
        if let Some(crate_bodies) = loaded_crate.bodies.get() {
            return Ok(crate_bodies.body(body_ref.body));
        }
        Ok(self
            .file_shard(body_ref.crate_ref, file)?
            .body(body_ref.body))
    }

    /// Find the body-local DefMap and item store paired with one body.
    ///
    /// Body data and body-local items are stored in the same file shard, so this follows the same
    /// manifest lookup as `body`.
    pub(super) fn body_local_items(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&BodyLocalItems>, PackageStoreError> {
        let Some((loaded_crate, file)) = self.body_location(body_ref)? else {
            return Ok(None);
        };
        if let Some(crate_bodies) = loaded_crate.bodies.get() {
            return Ok(crate_bodies.body_local_items(body_ref.body));
        }
        Ok(self
            .file_shard(body_ref.crate_ref, file)?
            .body_local_items(body_ref.body))
    }

    /// Resolve a stable body id to the crate state and file recorded by the manifest.
    fn body_location(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<(&LoadedCrate, FileId)>, PackageStoreError> {
        let loaded = self.loaded(body_ref.crate_ref.package)?;
        let Some(crate_manifest) = loaded.manifest.crate_manifest(body_ref.crate_ref.crate_id)
        else {
            return Ok(None);
        };
        let Some(file) = crate_manifest.body_file(body_ref.body) else {
            return Ok(None);
        };
        Ok(loaded
            .crate_data(body_ref.crate_ref.crate_id)
            .map(|crate_data| (crate_data, file)))
    }

    /// Load one declared file shard and keep it alive for the rest of the transaction.
    ///
    /// A missing crate or file means the loader and manifest disagree. Treat that as a stale
    /// package rather than quietly returning no bodies from a malformed cache revision.
    fn file_shard(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> Result<&BodyFileShard, PackageStoreError> {
        let loaded = self.loaded(crate_ref.package)?;
        let Some(loaded_crate) = loaded.crate_data(crate_ref.crate_id) else {
            return Err(PackageStoreError::stale_package(
                crate_ref.package,
                format!(
                    "Body IR crate {:?} is absent from its manifest",
                    crate_ref.crate_id
                ),
            ));
        };
        let Some((_, shard)) = loaded_crate
            .shards
            .iter()
            .find(|(shard_file, _)| *shard_file == file)
        else {
            return Err(PackageStoreError::stale_package(
                crate_ref.package,
                format!("Body IR file {:?} is absent from its crate manifest", file),
            ));
        };
        if shard.get().is_none() {
            let loaded_shard =
                self.loader
                    .load_file_shard(crate_ref.package, crate_ref.crate_id, file)?;
            let _ = shard.set(loaded_shard);
        }
        Ok(shard
            .get()
            .expect("Body IR shard cell should be initialized after successful load"))
    }

    /// Load the package manifest and use it to allocate the request-local crate cells.
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

/// Manifest plus empty-or-loaded state for every crate declared by it.
#[derive(Debug, Clone)]
struct LoadedPackage {
    manifest: Arc<PackageBodiesManifest>,
    crates: Vec<LoadedCrate>,
}

impl LoadedPackage {
    fn new(manifest: Arc<PackageBodiesManifest>) -> Self {
        let crates = manifest
            .crates()
            .iter()
            .map(|crate_manifest| LoadedCrate {
                item_lookup_index: OnceLock::new(),
                shards: crate_manifest
                    .files()
                    .iter()
                    .copied()
                    .map(|file| (file, OnceLock::new()))
                    .collect(),
                bodies: OnceLock::new(),
            })
            .collect();
        Self { manifest, crates }
    }

    fn crate_data(&self, crate_id: CrateId) -> Option<&LoadedCrate> {
        self.crates.get(crate_id.0)
    }
}

/// Independently loadable pieces of one crate.
///
/// `bodies` is the complete fallback representation. Once it is present, query methods prefer it
/// over `item_lookup_index` and `shards`, but already-loaded smaller pieces remain harmless and keep
/// any references returned earlier by the transaction valid.
#[derive(Debug, Clone)]
struct LoadedCrate {
    item_lookup_index: OnceLock<Arc<ItemLookupIndex>>,
    shards: Vec<(FileId, OnceLock<Arc<BodyFileShard>>)>,
    bodies: OnceLock<Arc<CrateBodies>>,
}
