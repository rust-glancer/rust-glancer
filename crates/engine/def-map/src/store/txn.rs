//! Read transactions over frozen DefMap package data.
//!
//! A transaction keeps package slots aligned with the project database, but each slot may be fully
//! resident, lazily backed by an artifact, or excluded by a query subset. Exact accessors preserve
//! the storage boundary: package metadata comes from the compact directory and one crate lookup
//! decodes only that crate. [`DefMapReadTxn::package`] remains the explicit broad-access path.

use std::sync::Arc;

use crate::{CrateData, CrateDefMapManifest, DefMap, DefMapSource, ModuleOrigin, PackageDefMaps};
use rg_ir_model::{CrateId, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_std::{ExpectedUnique, UniqueVec};

use super::{DefMapLoader, lazy::PackageReadEntry};
use crate::PackageSlot;

/// Read-only DefMap access for one query transaction.
///
/// Lazily decoded crate maps are shared by accessors on this value and released with the transaction.
#[derive(Debug, Clone)]
pub struct DefMapReadTxn<'db> {
    packages: Vec<PackageReadEntry<'db>>,
}

impl<'db> DefMapReadTxn<'db> {
    /// Builds transaction entries without changing their package-slot indexes.
    ///
    /// Included packages use their resident value when available. An offloaded package receives a
    /// lazy entry, optionally seeded with the manifest retained during startup; excluded slots keep
    /// a sentinel so an accidental access reports [`PackageStoreError::ExcludedSlot`].
    pub(crate) fn from_store_entries(
        packages: impl IntoIterator<
            Item = (
                bool,
                Option<Arc<PackageDefMaps>>,
                Option<Arc<crate::PackageDefMapsManifest>>,
            ),
        >,
        loader: DefMapLoader<'db>,
    ) -> Self {
        Self {
            packages: packages
                .into_iter()
                .map(|(included, package, manifest)| {
                    if !included {
                        return PackageReadEntry::Excluded;
                    }
                    match package {
                        Some(package) => PackageReadEntry::Resident(package),
                        None => PackageReadEntry::Lazy(super::lazy::LazyPackage::new(
                            loader.clone(),
                            manifest,
                        )),
                    }
                })
                .collect(),
        }
    }

    /// Returns the broad package representation, loading every crate map when it is offloaded.
    ///
    /// Prefer exact accessors such as [`Self::crate_data`] when the caller names one crate.
    pub fn package(&self, package_slot: PackageSlot) -> Result<&PackageDefMaps, PackageStoreError> {
        match self.entry(package_slot)? {
            PackageReadEntry::Resident(package) => Ok(package),
            PackageReadEntry::Lazy(package) => package.package(package_slot),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    pub fn package_name(&self, package: PackageSlot) -> Result<&str, PackageStoreError> {
        match self.entry(package)? {
            PackageReadEntry::Resident(package) => Ok(package.package_name()),
            PackageReadEntry::Lazy(lazy) => Ok(lazy.manifest(package)?.package_name()),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns the Rust edition used by one frozen package.
    pub fn package_edition(
        &self,
        package_slot: PackageSlot,
    ) -> Result<rg_text::RustEdition, PackageStoreError> {
        match self.entry(package_slot)? {
            PackageReadEntry::Resident(package) => Ok(package.edition()),
            PackageReadEntry::Lazy(package) => Ok(package.manifest(package_slot)?.edition()),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns one crate's DefMap data without loading sibling targets from an offloaded package.
    pub fn crate_data(&self, crate_ref: CrateRef) -> Result<Option<&CrateData>, PackageStoreError> {
        match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(package) => Ok(package.crate_data(crate_ref.crate_id)),
            PackageReadEntry::Lazy(package) => package.crate_data(crate_ref),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns an offloaded crate's compact routing entry without loading its module scopes.
    ///
    /// Resident packages return `None`: their [`CrateData`] is already authoritative, so keeping a
    /// second manifest representation would add memory without making those reads cheaper.
    fn crate_manifest(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&CrateDefMapManifest>, PackageStoreError> {
        match self.entry(crate_ref.package)? {
            PackageReadEntry::Resident(_) => Ok(None),
            PackageReadEntry::Lazy(package) => Ok(package
                .manifest(crate_ref.package)?
                .crate_manifest(crate_ref.crate_id)),
            PackageReadEntry::Excluded => unreachable!("excluded entries fail in entry()"),
        }
    }

    /// Returns one crate def map by project-wide crate reference.
    pub fn def_map(&self, crate_ref: CrateRef) -> Result<Option<&DefMap>, PackageStoreError> {
        Ok(self.crate_data(crate_ref)?.map(CrateData::def_map))
    }

    /// Returns crate contexts whose module tree contains a package-local file.
    ///
    /// Offloaded packages answer from the compact file-routing directory. Resident packages scan
    /// their module origins because they do not retain a duplicate manifest.
    pub fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Result<Vec<CrateRef>, PackageStoreError> {
        let PackageReadEntry::Resident(def_map_package) = self.entry(package)? else {
            let PackageReadEntry::Lazy(lazy) = self.entry(package)? else {
                unreachable!("excluded entries fail in entry()")
            };
            return lazy.crates_for_file(package, file);
        };

        let mut crates = Vec::new();
        for (crate_idx, crate_data) in def_map_package.crates().iter().enumerate() {
            if crate_data
                .def_map()
                .modules()
                .iter()
                .any(|module| module.origin.contains_file(file))
            {
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

impl DefMapSource for DefMapReadTxn<'_> {
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let Some(crate_ref) = origin.as_crate_ref() else {
            return Ok(None);
        };
        self.def_map(crate_ref)
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, PackageStoreError> {
        if let Some(manifest) = self.crate_manifest(crate_ref)? {
            return Ok(manifest.is_proc_macro());
        }
        Ok(self
            .crate_data(crate_ref)?
            .is_some_and(CrateData::is_proc_macro))
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .crate_data(crate_ref)?
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        Ok(self
            .crate_data(crate_ref)?
            .map(|data| {
                data.extern_prelude()
                    .iter()
                    .map(|(name, module)| (name.to_string(), *module))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self.crate_data(crate_ref)?.and_then(|data| data.prelude()))
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, PackageStoreError> {
        if let Some(manifest) = self.crate_manifest(crate_ref)? {
            return Ok(manifest.item_lookup_dependencies().clone());
        }
        Ok(self
            .crate_data(crate_ref)?
            .map(CrateData::item_lookup_dependencies)
            .unwrap_or_default())
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self.crate_data(crate_ref)?.and_then(|data| {
            Some(ModuleRef {
                origin: DefMapRef::Crate(crate_ref),
                module: data.root_module()?,
            })
        }))
    }
}
