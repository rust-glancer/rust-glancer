//! Read transactions over frozen def-map package data.

use crate::{DefMap, DefMapSource, ModuleOrigin, PackageDefMaps};
use rg_ir_model::{CrateId, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};
use rg_parse::FileId;
use rg_std::ExpectedUnique;

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

    /// Find the saved module named by an inline-module path from current syntax.
    ///
    /// The first module is the unique crate module whose definition is `file`. Components such as
    /// `outer::inner` then follow inline child modules declared in that same file. This remains
    /// usable after edits move those modules away from their saved byte ranges, but it deliberately
    /// rejects new, renamed, and ambiguous module paths.
    pub fn module_for_inline_path<T>(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        inline_module_path: &[T],
    ) -> Result<Option<ModuleRef>, PackageStoreError>
    where
        T: AsRef<str>,
    {
        let Some(def_map) = self.def_map(crate_ref)? else {
            return Ok(None);
        };

        let mut file_module = ExpectedUnique::new();
        for (module_index, module) in def_map.modules().iter().enumerate() {
            let owns_definition_file = match module.origin {
                ModuleOrigin::Root { file_id } => file_id == file,
                ModuleOrigin::OutOfLine {
                    definition_file: Some(definition_file),
                    ..
                } => definition_file == file,
                ModuleOrigin::Synthetic { .. }
                | ModuleOrigin::Inline { .. }
                | ModuleOrigin::OutOfLine {
                    definition_file: None,
                    ..
                } => false,
            };
            if owns_definition_file {
                file_module.push(rg_ir_model::ModuleId(module_index));
            }
        }
        let Some(mut module_id) = file_module.into_option() else {
            return Ok(None);
        };

        for component in inline_module_path {
            let Some(module) = def_map.module(module_id) else {
                return Ok(None);
            };
            let mut child = ExpectedUnique::new();
            for (name, child_id) in &module.children {
                if name == component.as_ref() {
                    child.push(*child_id);
                }
            }
            let Some(child) = child.into_option() else {
                return Ok(None);
            };
            let Some(child_data) = def_map.module(child) else {
                return Ok(None);
            };
            if !matches!(
                child_data.origin,
                ModuleOrigin::Inline {
                    declaration_file,
                    ..
                } if declaration_file == file
            ) {
                return Ok(None);
            }
            module_id = child;
        }

        Ok(Some(ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module: module_id,
        }))
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

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, PackageStoreError> {
        Ok(self
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .is_some_and(crate::CrateData::is_proc_macro))
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
