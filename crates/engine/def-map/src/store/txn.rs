//! Read transactions over frozen def-map package data.

use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use crate::{
    DefMap, GeneratedItemRef, LocalDefData, LocalDefRef, LocalImplData, LocalImplRef, ModuleRef,
    PackageDefMaps, PackageSlot, Path, ResolvePathResult, TargetRef, query::path_resolution,
};

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

    /// Returns one local definition by stable project-wide reference.
    pub fn local_def(
        &self,
        local_def: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, PackageStoreError> {
        Ok(self
            .def_map(local_def.target)?
            .and_then(|def_map| def_map.local_def(local_def.local_def)))
    }

    /// Returns one impl block by stable project-wide reference.
    pub fn local_impl(
        &self,
        local_impl: LocalImplRef,
    ) -> Result<Option<&LocalImplData>, PackageStoreError> {
        Ok(self
            .def_map(local_impl.target)?
            .and_then(|def_map| def_map.local_impl(local_impl.local_impl)))
    }

    /// Returns one retained generated item by stable target-local reference.
    pub fn generated_item(
        &self,
        target: TargetRef,
        item: GeneratedItemRef,
    ) -> Result<Option<&rg_item_tree::ItemNode>, PackageStoreError> {
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.generated_source(item.source))
            .and_then(|source| source.item(item.item)))
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
