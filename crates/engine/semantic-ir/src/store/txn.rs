//! Read transactions over frozen Semantic IR package data.

use rg_def_map::PackageSlot;
use rg_ir_model::{DefMapRef, TargetRef};
use rg_ir_storage::{ItemStore, ItemStoreSource};
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

    pub fn items(&self, target: TargetRef) -> Result<Option<&ItemStore>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.target(target.target))
    }

    pub fn included_stores(&self) -> Result<Vec<&ItemStore>, PackageStoreError> {
        let mut target_stores = Vec::new();

        for package in self.packages.included_packages() {
            target_stores.extend(package?.targets().iter())
        }
        Ok(target_stores)
    }
}

impl<'a, 'db> ItemStoreSource<'a> for &'a SemanticIrReadTxn<'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        let Some(target) = origin.as_target_ref() else {
            return Ok(None);
        };

        (*self).items(target)
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        (*self).included_stores()
    }
}
