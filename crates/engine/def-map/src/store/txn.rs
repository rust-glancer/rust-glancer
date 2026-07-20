//! Read transactions over frozen def-map package data.

use crate::{DefMap, DefMapSource, PackageDefMaps};
use rg_ir_model::{CrateId, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};
use rg_parse::FileId;

use crate::PackageSlot;

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

    /// Returns the Rust edition used by one frozen package.
    pub fn package_edition(
        &self,
        package_slot: PackageSlot,
    ) -> Result<rg_text::RustEdition, PackageStoreError> {
        Ok(self.package(package_slot)?.edition())
    }

    /// Returns one crate def map by project-wide crate reference.
    pub fn def_map(&self, crate_ref: CrateRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let package = self.package(crate_ref.package)?;
        Ok(package.def_map(crate_ref.crate_id))
    }

    /// Returns crate contexts whose module tree contains a package-local file.
    pub fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Result<Vec<CrateRef>, PackageStoreError> {
        let mut crates = Vec::new();
        let def_map_package = self.package(package)?;

        for (crate_idx, crate_data) in def_map_package.crates().iter().enumerate() {
            let def_map = crate_data.def_map();
            let owns_file = def_map
                .modules()
                .iter()
                .any(|module| module.origin.contains_file(file));
            if owns_file {
                crates.push(CrateRef {
                    package,
                    crate_id: CrateId(crate_idx),
                });
            }
        }

        Ok(crates)
    }
}

impl DefMapSource for DefMapReadTxn<'_> {
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let Some(crate_ref) = origin.as_crate_ref() else {
            return Ok(None);
        };
        self.def_map(crate_ref)
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        Ok(self
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .map(|data| {
                data.extern_prelude()
                    .iter()
                    .map(|(name, module)| (name.to_string(), *module))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| {
                Some(ModuleRef {
                    origin: DefMapRef::Crate(crate_ref),
                    module: data.root_module()?,
                })
            }))
    }
}
