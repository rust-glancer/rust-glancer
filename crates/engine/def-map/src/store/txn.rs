//! Read transactions over frozen def-map package data.

use rg_ir_model::{DefMapRef, ModuleRef, TargetRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use crate::{
    DefMap, DefMapQuery, DefMapSource, PackageDefMaps, PackageSlot, Path, ResolvePathResult,
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

    /// Resolves a value-position path from one module against this transaction.
    pub fn resolve_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        DefMapQuery::new(self).resolve_path(from, path)
    }

    /// Resolves a type-position path from one module against this transaction.
    pub fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        DefMapQuery::new(self).resolve_path_in_type_namespace(from, path)
    }
}

impl DefMapSource for DefMapReadTxn<'_> {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let Some(target) = origin.as_target_ref() else {
            return Ok(None);
        };
        self.def_map(target)
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(target.package)?
            .target_data(target.target)
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(target.package)?
            .target_data(target.target)
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(target.package)?
            .target_data(target.target)
            .and_then(|data| {
                Some(ModuleRef {
                    origin: DefMapRef::Target(target),
                    module: data.root_module()?,
                })
            }))
    }
}
