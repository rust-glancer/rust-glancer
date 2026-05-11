//! Read transactions over frozen def-map package data.

use rg_package_store::{PackageRead, PackageStoreError, PackageStoreReadTxn};
use rg_parse::TargetId;

use crate::{
    DefMap, ImportData, ImportId, ImportRef, LocalDefData, LocalDefId, LocalDefRef, LocalImplData,
    LocalImplRef, ModuleData, ModuleId, ModuleRef, Package, PackageSlot, Path, ResolvePathResult,
    TargetRef, path_resolution,
};

/// Read-only def-map access for one query transaction.
#[derive(Debug, Clone)]
pub struct DefMapReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, Package>,
}

impl<'db> DefMapReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, Package>) -> Self {
        Self { packages }
    }

    /// Returns one package by package slot.
    pub fn package(
        &self,
        package_slot: PackageSlot,
    ) -> Result<PackageRead<'_, Package>, PackageStoreError> {
        self.packages.read(package_slot)
    }

    /// Returns one target def map by project-wide target reference.
    pub fn def_map(&self, target: TargetRef) -> Result<Option<&DefMap>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.into_ref().target(target.target))
    }

    /// Materializes every included target def map with its project-wide target reference.
    pub fn materialize_included_target_maps(
        &self,
    ) -> Result<Vec<(TargetRef, &DefMap)>, PackageStoreError> {
        let target_maps = self
            .packages
            .materialize_included_packages_with_slots()?
            .into_iter()
            .flat_map(move |(package_slot, package)| {
                let package = package.into_ref();
                package
                    .targets()
                    .iter()
                    .enumerate()
                    .map(move |(target_idx, def_map)| {
                        let target_ref = TargetRef {
                            package: package_slot,
                            target: TargetId(target_idx),
                        };
                        (target_ref, def_map)
                    })
            })
            .collect::<Vec<_>>();

        Ok(target_maps)
    }

    /// Iterates over one target's modules together with stable project-wide references.
    pub fn modules(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(ModuleRef, &ModuleData)>, PackageStoreError> {
        let modules = self
            .def_map(target)?
            .into_iter()
            .flat_map(move |def_map| {
                def_map
                    .modules()
                    .iter()
                    .enumerate()
                    .map(move |(module_idx, module)| {
                        (
                            ModuleRef {
                                target,
                                module: ModuleId(module_idx),
                            },
                            module,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(modules)
    }

    /// Returns one module by stable project-wide reference.
    pub fn module(&self, module: ModuleRef) -> Result<Option<&ModuleData>, PackageStoreError> {
        Ok(self
            .def_map(module.target)?
            .and_then(|def_map| def_map.module(module.module)))
    }

    /// Iterates over one target's local definitions together with stable project-wide references.
    pub fn local_defs(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(LocalDefRef, &LocalDefData)>, PackageStoreError> {
        let local_defs =
            self.def_map(target)?
                .into_iter()
                .flat_map(move |def_map| {
                    def_map.local_defs().iter().enumerate().map(
                        move |(local_def_idx, local_def)| {
                            (
                                LocalDefRef {
                                    target,
                                    local_def: LocalDefId(local_def_idx),
                                },
                                local_def,
                            )
                        },
                    )
                })
                .collect::<Vec<_>>();

        Ok(local_defs)
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

    /// Iterates over one target's impl blocks together with stable project-wide references.
    pub fn local_impls(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(LocalImplRef, &LocalImplData)>, PackageStoreError> {
        let local_impls =
            self.def_map(target)?
                .into_iter()
                .flat_map(move |def_map| {
                    def_map.local_impls().iter().enumerate().map(
                        move |(local_impl_idx, local_impl)| {
                            (
                                LocalImplRef {
                                    target,
                                    local_impl: crate::LocalImplId(local_impl_idx),
                                },
                                local_impl,
                            )
                        },
                    )
                })
                .collect::<Vec<_>>();

        Ok(local_impls)
    }

    /// Iterates over one target's imports together with stable project-wide references.
    pub fn imports(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(ImportRef, &ImportData)>, PackageStoreError> {
        let imports = self
            .def_map(target)?
            .into_iter()
            .flat_map(move |def_map| {
                def_map
                    .imports()
                    .iter()
                    .enumerate()
                    .map(move |(import_idx, import)| {
                        (
                            ImportRef {
                                target,
                                import: ImportId(import_idx),
                            },
                            import,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(imports)
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
