//! Read transactions over frozen Semantic IR package data.

use crate::{ItemLookupIndex, ItemLookupIndexSource, ItemStore, ItemStoreSource};
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateRef, DefMapRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use crate::PackageIr;

/// Read-only semantic IR access for one query transaction.
#[derive(Debug, Clone)]
pub struct SemanticIrReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageIr>,
}

impl<'db> SemanticIrReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageIr>) -> Self {
        Self { packages }
    }

    pub fn package(&self, package: PackageSlot) -> Result<&PackageIr, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn items(&self, crate_ref: CrateRef) -> Result<Option<&ItemStore>, PackageStoreError> {
        let package = self.package(crate_ref.package)?;
        Ok(package.crate_items(crate_ref.crate_id))
    }

    /// Return the declaration-local lookup index for one semantic crate.
    ///
    /// This access does not compose visible dependencies. Callers that perform use-site lookup pass
    /// the returned indexes through [`crate::ItemLookupQuery`].
    pub fn item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&ItemLookupIndex>, PackageStoreError> {
        let package = self.package(crate_ref.package)?;
        Ok(package.crate_lookup_index(crate_ref.crate_id))
    }

    pub fn included_stores(&self) -> Result<Vec<&ItemStore>, PackageStoreError> {
        let mut stores = Vec::new();

        for package in self.packages.included_packages() {
            stores.extend(package?.crates().iter())
        }
        Ok(stores)
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
