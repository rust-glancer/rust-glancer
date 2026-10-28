//! Lookup storage while DefMap scopes are being built.
//!
//! Dirty packages read fresh module builders; unchanged packages use lazy, fallible frozen reads.
//! The shared `NameResolver` supplies Rust lookup rules. This adapter only routes its reads and
//! borrows the builders immutably; the import collector supplies scope buffers while updating them.
//!
//! During import collection, the collector reads its lent scopes first and delegates other reads
//! here. During macro lookup, all buffers are back in their builders, so this environment can be
//! used directly. A missing name in a dirty crate never falls back to that crate's old frozen data.

use rg_ir_model::{
    CrateRef, DefId, DefMapRef, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleId, ModuleRef,
};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use super::{
    crates::CrateBuildStore,
    imports::{ImportCollectionEnv, ModuleImportInput},
};
use crate::{
    CrateResolutionEnv, DefMapReadTxn, DefMapSource, ImportData, LocalDefData,
    LocalEnumVariantData, LocalEnumVariantEntry, MacroDefinitionEnv, MacroDefinitionView,
    ModuleBuilder, ModuleData, ModuleResolutionView, NameBinding, NameBindingsView,
    NameResolutionEnv, Namespace,
};

/// Reads fresh crate builders alongside unchanged packages from the frozen baseline.
///
/// This type supplies storage access, not import scheduling. Its extra import methods expose
/// declaration lists and the proc-macro export rule to the collector using the same builders.
pub(crate) struct BuildResolutionEnv<'a> {
    baseline: &'a DefMapReadTxn<'a>,
    crates: &'a CrateBuildStore,
}

impl<'a> BuildResolutionEnv<'a> {
    pub(crate) fn new(baseline: &'a DefMapReadTxn<'a>, crates: &'a CrateBuildStore) -> Self {
        Self { baseline, crates }
    }
}

impl NameResolutionEnv for BuildResolutionEnv<'_> {
    type Error = PackageStoreError;

    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, Self::Error> {
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
    ) -> Result<Option<NameBindingsView<'a>>, Self::Error> {
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
            .map(|entry| entry.as_view()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, NameBindingsView<'a>)>, Self::Error> {
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
                    .map(|(name, entry)| (name, entry.as_view()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, Self::Error> {
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
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
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
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
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
    ) -> Result<Option<MacroDefinitionView<'a>>, Self::Error> {
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
    ) -> Result<Option<ModuleRef>, Self::Error> {
        if let Some(state) = self.crates.crate_builder(crate_ref) {
            return Ok(state.extern_prelude.resolve(name));
        }

        Ok(self
            .baseline
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        if let Some(state) = self.crates.crate_builder(crate_ref) {
            return Ok(state.prelude);
        }

        Ok(self
            .baseline
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
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

impl ImportCollectionEnv for BuildResolutionEnv<'_> {
    fn import_modules(&self) -> Vec<ModuleImportInput<'_>> {
        let mut result = Vec::new();
        for states in self.crates.iter_dirty() {
            for state in states {
                for index in 0..state.def_map_builder.module_count() {
                    let module = ModuleId(index);
                    let data = state
                        .def_map_builder
                        .module(module)
                        .expect("collected module exists");
                    if data.imports.is_empty() {
                        continue;
                    }
                    let reference = ModuleRef::krate(state.crate_ref, module);
                    result.push(ModuleImportInput {
                        module: reference,
                        imports: &data.imports,
                    });
                }
            }
        }
        result
    }

    fn import_data(&self, import: ImportRef) -> &ImportData {
        let krate = import
            .origin
            .as_crate_ref()
            .expect("crate imports have a crate origin");
        &self
            .crates
            .crate_builder(krate)
            .expect("import owner is dirty")
            .def_map_builder
            .imports()[import.import.0]
    }

    fn allow_public_binding(
        &self,
        module: ModuleRef,
        namespace: Namespace,
        binding: &NameBinding,
    ) -> bool {
        module
            .origin
            .as_crate_ref()
            .and_then(|krate| self.crates.crate_builder(krate))
            .is_none_or(|state| state.allow_public_binding(module.module, namespace, binding))
    }
}
