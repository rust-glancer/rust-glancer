//! Read transactions over frozen def-map package data.

use rg_ir_model::{ModuleRef, TargetRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use crate::{DefMap, PackageDefMaps, PackageSlot, Path, ResolvePathResult, query::path_resolution};

/// Read-only def-map access for one query transaction.
#[derive(Debug, Clone)]
pub struct DefMapReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageDefMaps>,
}

impl<'db> DefMapReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageDefMaps>) -> Self {
        Self { packages }
    }

    /// Returns one package by package slot.
    pub fn package(&self, package_slot: PackageSlot) -> Result<&PackageDefMaps, PackageStoreError> {
        self.packages.read(package_slot)
    }

    /// Returns one target def map by project-wide target reference.
    pub fn def_map(&self, target: TargetRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.def_map(target.target))
    }

    /// Resolves a value-position path from one module against this transaction.
    pub fn resolve_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        path_resolution::resolve_path_in_txn(self, from, path)
    }

    /// Resolves a type-position path from one module against this transaction.
    pub fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        path_resolution::resolve_path_in_type_namespace_txn(self, from, path)
    }
}
