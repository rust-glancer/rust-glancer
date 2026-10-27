//! Lookup storage while DefMap scopes are being built.
//!
//! Dirty packages read fresh module builders; unchanged packages use lazy, fallible frozen reads.
//! The shared `ScopeResolver` supplies Rust lookup rules. This adapter only routes its reads and
//! borrows the builders immutably so one import wave sees a stable set of input scopes.

use rg_ir_model::{CrateRef, DefId, DefMapRef, LocalDefRef, LocalEnumVariantRef, ModuleRef};
use rg_text::Name;

use super::crates::CrateBuildStore;
use crate::{
    CrateResolutionEnv, DefMapReadTxn, DefMapSource, LocalDefData, LocalEnumVariantData,
    LocalEnumVariantEntry, MacroDefinitionEnv, MacroDefinitionView, ModuleBuilder, ModuleData,
    ModuleResolutionView, ModuleScopeBuilder, ScopeEntryRef, ScopeResolutionEnv,
};

/// Resolution environment used while dirty package scopes are being fixed up.
///
/// Dirty package reads come from fresh crate state and the current fixed-point module scopes.
/// Clean package reads fall through to the frozen baseline.
pub(crate) struct BuildResolutionEnv<'a> {
    baseline: &'a DefMapReadTxn<'a>,
    crates: &'a CrateBuildStore,
}

impl<'a> BuildResolutionEnv<'a> {
    pub(crate) fn new(baseline: &'a DefMapReadTxn<'a>, crates: &'a CrateBuildStore) -> Self {
        Self { baseline, crates }
    }

    /// Return the input scope a module worker uses to decide whether its candidate changed.
    pub(crate) fn current_module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.crates.module_scope(module)
    }
}

impl ScopeResolutionEnv for BuildResolutionEnv<'_> {
    type Error = rg_package_store::PackageStoreError;

    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = module_ref.origin.as_crate_ref()
            && let Some(state) = self.crates.crate_builder(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .module(module_ref.module)
                .map(ModuleBuilder::resolution_view));
        }

        let Some(crate_ref) = module_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .baseline
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.module(module_ref.module))
            .map(ModuleData::resolution_view))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, rg_package_store::PackageStoreError> {
        if module_ref
            .origin
            .as_crate_ref()
            .is_some_and(|crate_ref| self.crates.package(crate_ref.package).is_some())
        {
            return Ok(self
                .crates
                .module_scope(module_ref)
                .and_then(|scope| scope.entry(name)));
        }

        Ok(self
            .baseline
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, rg_package_store::PackageStoreError> {
        if module_ref
            .origin
            .as_crate_ref()
            .is_some_and(|crate_ref| self.crates.package(crate_ref.package).is_some())
        {
            return Ok(self
                .crates
                .module_scope(module_ref)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }

        Ok(self
            .baseline
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
    ) -> Result<Option<&LocalDefData>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = local_def_ref.origin.as_crate_ref()
            && let Some(state) = self.crates.crate_builder(crate_ref)
        {
            return Ok(state.def_map_builder.local_def(local_def_ref.local_def));
        }

        let Some(crate_ref) = local_def_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .baseline
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = variant_ref.origin.as_crate_ref()
            && let Some(state) = self.crates.crate_builder(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant(variant_ref.local_enum_variant));
        }

        let Some(crate_ref) = variant_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .baseline
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.local_enum_variant(variant_ref.local_enum_variant)))
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(state) = self.crates.crate_builder(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect());
        }

        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(def_map) = self.baseline.def_map(crate_ref)?
        {
            Ok(def_map
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

impl MacroDefinitionEnv for BuildResolutionEnv<'_> {
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, rg_package_store::PackageStoreError> {
        let DefId::Local(def_ref) = def else {
            return Ok(None);
        };
        let Some(local_def) = self.local_def_data(def_ref)? else {
            return Ok(None);
        };

        let data = if let Some(crate_ref) = def_ref.origin.as_crate_ref()
            && let Some(state) = self.crates.crate_builder(crate_ref)
        {
            state.def_map_builder.macro_definition(def_ref.local_def)
        } else {
            let Some(crate_ref) = def_ref.origin.as_crate_ref() else {
                return Ok(None);
            };
            self.baseline
                .def_map(crate_ref)?
                .and_then(|def_map| def_map.macro_definition(def_ref.local_def))
        };
        let Some(data) = data else {
            return Ok(None);
        };

        Ok(MacroDefinitionView::new(def_ref, local_def, data))
    }
}

impl CrateResolutionEnv for BuildResolutionEnv<'_> {
    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.crates.crate_builder(crate_ref) {
            return Ok(state.extern_prelude.resolve(name));
        }

        Ok(self
            .baseline
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn prelude_module(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.crates.crate_builder(crate_ref) {
            return Ok(state.prelude);
        }

        Ok(self
            .baseline
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.prelude()))
    }

    fn root_module(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        let module = if let Some(state) = self.crates.crate_builder(crate_ref) {
            Some(state.root_module)
        } else {
            self.baseline
                .package(crate_ref.package)?
                .crate_data(crate_ref.crate_id)
                .and_then(|data| data.root_module())
        };

        Ok(module.map(|module| ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module,
        }))
    }
}
