//! Read transactions over frozen Semantic IR package data.
//!
//! A package slot may be resident, lazily backed by an artifact, or excluded by a query subset.
//! Exact accessors preserve the artifact boundaries between declarations and lookup indexes;
//! [`SemanticIrReadTxn::package`] is the explicit path that reconstructs the broad package value.

use std::sync::Arc;

use crate::{CrateIr, ItemLookupIndex, ItemLookupIndexSource, ItemStore, ItemStoreSource};
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateRef, DefMapRef};
use rg_package_store::PackageStoreError;

use super::{SemanticIrLoader, lazy::PackageReadEntry};
use crate::PackageIr;

/// Read-only Semantic IR access for one query transaction.
///
/// Lazily decoded crate parts are shared by accessors on this value and released with the transaction.
#[derive(Debug, Clone)]
pub struct SemanticIrReadTxn<'db> {
    packages: Vec<PackageReadEntry<'db>>,
}

impl<'db> SemanticIrReadTxn<'db> {
    /// Builds transaction entries without changing their package-slot indexes.
    ///
    /// Included packages use a resident value when present and otherwise receive a lazy artifact
    /// view. Excluded slots keep a sentinel so accidental access reports an excluded-slot error.
    pub(crate) fn from_store_entries(
        packages: impl IntoIterator<Item = (bool, Option<Arc<PackageIr>>)>,
        loader: SemanticIrLoader<'db>,
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
                        None => {
                            PackageReadEntry::Lazy(super::lazy::LazyPackage::new(loader.clone()))
                        }
                    }
                })
                .collect(),
        }
    }

    /// Returns the broad package representation, loading both parts of every offloaded crate.
    ///
    /// Prefer [`Self::items`] or [`Self::item_lookup_index`] when the caller names one crate.
    pub fn package(&self, package: PackageSlot) -> Result<&PackageIr, PackageStoreError> {
        match self.entry(package)? {
            PackageReadEntry::Resident(package_ir) => Ok(package_ir),
            PackageReadEntry::Lazy(package_ir) => package_ir.package(package),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns one crate's declarations without loading its lookup index or sibling crates.
    pub fn items(&self, crate_ref: CrateRef) -> Result<Option<&ItemStore>, PackageStoreError> {
        match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => Ok(package.crate_items(crate_ref.crate_id)),
            PackageReadEntry::Lazy(package) => package.items(crate_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Return the declaration-local lookup index for one semantic crate.
    ///
    /// This access does not compose visible dependencies. Callers that perform use-site lookup pass
    /// the returned indexes through [`crate::ItemLookupQuery`].
    pub fn item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&ItemLookupIndex>, PackageStoreError> {
        match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => {
                Ok(package.crate_lookup_index(crate_ref.crate_id))
            }
            PackageReadEntry::Lazy(package) => package.lookup_index(crate_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns every declaration store included in this transaction.
    ///
    /// This is an intentionally broad compatibility path. For lazy packages it decodes the item
    /// store of every crate, so exact query code should use [`Self::items`] instead.
    pub fn included_stores(&self) -> Result<Vec<&ItemStore>, PackageStoreError> {
        let mut stores = Vec::new();

        for (package_idx, entry) in self.packages.iter().enumerate() {
            let package = PackageSlot(package_idx);
            match entry {
                PackageReadEntry::Resident(package) => {
                    stores.extend(package.crates().iter().map(CrateIr::items));
                }
                PackageReadEntry::Lazy(lazy) => {
                    for crate_idx in 0..lazy.manifest(package)?.crate_count() {
                        let crate_ref = CrateRef {
                            package,
                            crate_id: rg_ir_model::CrateId(crate_idx),
                        };
                        if let Some(items) = lazy.items(crate_ref)? {
                            stores.push(items);
                        }
                    }
                }
                PackageReadEntry::Excluded => {}
            }
        }
        Ok(stores)
    }

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

impl<'a, 'db> ItemStoreSource<'a> for &'a SemanticIrReadTxn<'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        let Some(crate_ref) = origin.as_crate_ref() else {
            return Ok(None);
        };

        (*self).items(crate_ref)
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        (*self).included_stores()
    }
}

impl<'a, 'db> ItemLookupIndexSource<'a> for &'a SemanticIrReadTxn<'db> {
    fn item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&'a ItemLookupIndex>, PackageStoreError> {
        (*self).item_lookup_index(crate_ref)
    }
}
