//! Read transactions over frozen def-map package data.

use rg_ir_model::{DefMapRef, LocalDefRef, ModuleRef, TargetRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};
use rg_text::Name;

use crate::{
    DefMap, LocalDefData, MacroDefinitionData, ModuleData, PackageDefMaps, PackageSlot, Path,
    ResolvePathResult,
    model::ScopeEntryRef,
    query::{
        path_resolution::{NameResolutionFilter, PathResolver},
        resolution_env::{ScopeResolutionEnv, TargetResolutionEnv},
    },
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
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::AllNamespaces)
    }

    /// Resolves a type-position path from one module against this transaction.
    pub fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::TypesOnly)
    }
}

impl TargetResolutionEnv for DefMapReadTxn<'_> {
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.target_data().extern_prelude().get(name).copied()))
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.target_data().prelude()))
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self.def_map(target)?.and_then(|def_map| {
            Some(ModuleRef {
                origin: DefMapRef::Target(target),
                module: def_map.target_data().root_module()?,
            })
        }))
    }
}

impl ScopeResolutionEnv for DefMapReadTxn<'_> {
    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, PackageStoreError> {
        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, PackageStoreError> {
        Ok(self
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, PackageStoreError> {
        Ok(self
            .module_data(module_ref)?
            .map(|module| {
                module
                    .scope
                    .entries()
                    .map(|(name, entry)| (name, entry.as_ref()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, PackageStoreError> {
        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, PackageStoreError> {
        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }
}
