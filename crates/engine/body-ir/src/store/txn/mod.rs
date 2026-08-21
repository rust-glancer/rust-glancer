//! Query-facing access to resident and offloaded Body IR.
//!
//! The retained database still uses the generic package store to record package slots and
//! residency. Its generic read transaction would have to load an entire [`PackageBodies`] value,
//! though, which would throw away Body IR's source-file cache granularity. [`BodyIrReadTxn`] is the
//! Body-specific read view that keeps the same logical package slots while loading smaller units.
//!
//! Callers do not branch on residency. The transaction does that once and then exposes Body IR
//! operations:
//!
//! ```text
//! bodies(crate_ref, Some(file)) -> one source-file shard
//! body(body_ref)                -> the shard named by the manifest
//! crate_bodies(crate_ref)       -> complete crate
//! ```
//!
//! Values loaded for an offloaded package are owned by the transaction. This is why these methods
//! can return borrowed Body IR references without making the package resident in `BodyIrDb`.

mod lazy;
mod loader;

use std::sync::Arc;

use rg_def_map::DefMap;
use rg_def_map::PackageSlot;
use rg_ir_model::{BodyId, BodyRef, CrateRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_semantic_ir::ItemStore;

use self::lazy::{LazyPackage, PackageReadEntry};
pub use self::loader::{BodyIrLoader, LoadBodyIr};
use crate::{BodyLocalItems, BodyView, CrateBodies, CurrentBodySet, PackageBodies};

/// Read-only Body IR access with one stable view of package residency and loaded cache units.
///
/// Cloning the transaction shares resident `Arc`s and clones any request-local loaded `Arc`s. It
/// does not change the retained database or turn an offloaded package into a resident one.
#[derive(Debug, Clone)]
pub struct BodyIrReadTxn<'db> {
    packages: Vec<PackageReadEntry<'db>>,
    current: Arc<CurrentBodySet>,
}

impl<'db> BodyIrReadTxn<'db> {
    /// Freeze the package subset and residency states used by this query.
    ///
    /// `Some(package)` becomes a resident entry. An included `None` becomes a lazy Body IR entry,
    /// while an excluded slot stays distinguishable from a missing package slot.
    pub(crate) fn from_store_entries(
        packages: impl IntoIterator<Item = (bool, Option<Arc<PackageBodies>>)>,
        loader: BodyIrLoader<'db>,
    ) -> Self {
        Self {
            packages: packages
                .into_iter()
                .map(|(included, package)| {
                    if !included {
                        return PackageReadEntry::Excluded;
                    }
                    match package {
                        Some(package) => PackageReadEntry::Resident(package),
                        None => PackageReadEntry::Lazy(LazyPackage::new(loader.clone())),
                    }
                })
                .collect(),
            current: Arc::new(CurrentBodySet::default()),
        }
    }

    /// Make this read transaction use request-local bodies for the selected files.
    pub fn with_current_body_set(mut self, current: CurrentBodySet) -> Self {
        self.current = Arc::new(current);
        self
    }

    /// Return whether this body was rebuilt from the request's current source.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.current.contains_body(body_ref)
    }

    /// Allocate the first body id that cannot collide with a saved body in this crate.
    ///
    /// A rebuilt body still needs an id when early-start indexing did not keep any saved bodies.
    /// This id is used only inside the request and is never written to retained Body IR.
    pub fn first_synthetic_body_ref(
        &self,
        crate_ref: CrateRef,
    ) -> Result<BodyRef, PackageStoreError> {
        let body_count = match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => package
                .crate_bodies(crate_ref.crate_id)
                .map(|bodies| bodies.bodies().len())
                .unwrap_or_default(),
            PackageReadEntry::Lazy(package) => package.body_count(crate_ref)?.unwrap_or_default(),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        };
        Ok(BodyRef {
            crate_ref,
            body: BodyId(body_count),
        })
    }

    /// Return the complete crate, loading every required Body IR storage unit when offloaded.
    ///
    /// Prefer the narrower query methods when the caller needs one body or one file.
    pub fn crate_bodies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&CrateBodies>, PackageStoreError> {
        if self.current.affects_crate(crate_ref) {
            // A complete saved `CrateBodies` value cannot represent request-local replacements or
            // a whole-file mask. Callers that use current bodies must use the narrower methods.
            return Ok(None);
        }
        match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => Ok(package.crate_bodies(crate_ref.crate_id)),
            PackageReadEntry::Lazy(package) => package.crate_bodies(crate_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Enumerate bodies from one file, or every body when `file` is absent.
    ///
    /// Returning stable `BodyRef` values here keeps file-local scanners out of the physical shard
    /// layout and prevents them from accidentally requesting the rest of a large crate.
    pub fn bodies(
        &self,
        crate_ref: CrateRef,
        file: Option<FileId>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, PackageStoreError> {
        let mut saved = match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => package
                .crate_bodies(crate_ref.crate_id)
                .map(|crate_bodies| {
                    crate_bodies
                        .body_views()
                        .filter(|(_, body)| file.is_none_or(|file| body.source().file_id == file))
                        .map(|(body, view)| (BodyRef { crate_ref, body }, view))
                        .collect()
                })
                .unwrap_or_default(),
            PackageReadEntry::Lazy(package) => package.bodies(crate_ref, file)?,
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        };

        saved.retain(|(body_ref, body)| {
            !self.current.contains_body(*body_ref)
                && !self.current.masks_file(crate_ref, body.source().file_id)
        });

        // Add rebuilt bodies after removing their saved identities and any saved bodies masked by
        // different editor text. Their expressions and locals come from the current source.
        for current in self.current.bodies().iter().filter(|body| {
            body.body_ref().crate_ref == crate_ref
                && file.is_none_or(|file| body.view().source().file_id == file)
        }) {
            saved.push((current.body_ref(), current.view()));
        }

        Ok(saved)
    }

    /// Return one body by project-wide body reference.
    ///
    /// For an offloaded package, the manifest maps the `BodyId` to its source file and only that
    /// file shard is loaded.
    pub fn body(&self, body_ref: BodyRef) -> Result<Option<BodyView<'_>>, PackageStoreError> {
        if let Some(body) = self
            .current
            .bodies()
            .iter()
            .find(|body| body.body_ref() == body_ref)
        {
            return Ok(Some(body.view()));
        }

        let saved = match self.entry(body_ref.crate_ref.package)? {
            PackageReadEntry::Resident(package) => Ok(package
                .crate_bodies(body_ref.crate_ref.crate_id)
                .and_then(|crate_bodies| crate_bodies.body(body_ref.body))),
            PackageReadEntry::Lazy(package) => package.body(body_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }?;
        Ok(saved.filter(|body| {
            !self
                .current
                .masks_file(body_ref.crate_ref, body.source().file_id)
        }))
    }

    /// Return the DefMap and item store created inside one body.
    ///
    /// These values are paired with the body in the same file shard, so the lookup has the same
    /// narrow loading behavior as `body`.
    pub fn body_local_items(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&BodyLocalItems>, PackageStoreError> {
        if let Some(body) = self
            .current
            .bodies()
            .iter()
            .find(|body| body.body_ref() == body_ref)
        {
            return Ok(Some(body.local_items()));
        }

        if self.body(body_ref)?.is_none() {
            return Ok(None);
        }

        match self.entry(body_ref.crate_ref.package)? {
            PackageReadEntry::Resident(package) => Ok(package
                .crate_bodies(body_ref.crate_ref.crate_id)
                .and_then(|crate_bodies| crate_bodies.body_local_items(body_ref.body))),
            PackageReadEntry::Lazy(package) => package.body_local_items(body_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Return the body-local DefMap without exposing the containing storage object.
    pub fn body_def_map(&self, body_ref: BodyRef) -> Result<Option<&DefMap>, PackageStoreError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::def_map))
    }

    /// Return the body-local item store without exposing the containing storage object.
    pub fn body_item_store(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&ItemStore>, PackageStoreError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::item_store))
    }

    /// Distinguish an invalid slot from a valid slot omitted by this transaction's subset.
    fn entry(&self, package: PackageSlot) -> Result<&PackageReadEntry<'db>, PackageStoreError> {
        let Some(entry) = self.packages.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };
        if matches!(entry, PackageReadEntry::Excluded) {
            return Err(PackageStoreError::ExcludedSlot { slot: package });
        }
        Ok(entry)
    }
}
