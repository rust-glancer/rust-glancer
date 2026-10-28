//! Lookup storage while DefMap scopes are being built.
//!
//! The active crate reads its builder. Completed dependencies read frozen data; unchanged packages
//! use lazy, fallible baseline reads.
//! The shared `NameResolver` supplies Rust lookup rules. This adapter only routes its reads and
//! borrows the active builder immutably. The import collector supplies scope buffers while updating
//! them.
//!
//! During import collection, the collector reads its lent scopes first and delegates other reads
//! here. During macro lookup, all buffers are back in their builders, so this environment can be
//! used directly. Selected crates never fall back to their old frozen data.

use rg_ir_model::{
    CrateRef, DefId, DefMapRef, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleId, ModuleRef,
};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use super::{
    crates::CrateBuilder,
    imports::{ImportCollectionEnv, ModuleImportInput},
    output::CompletedCrateStore,
};
use crate::{
    CrateData, CrateResolutionEnv, DefMapReadTxn, ImportData, LocalDefData, LocalEnumVariantData,
    LocalEnumVariantEntry, MacroDefinitionEnv, MacroDefinitionView, ModuleBuilder, ModuleData,
    ModuleResolutionView, NameBinding, NameBindingsView, NameResolutionEnv, Namespace,
};

/// Reads the active builder alongside completed and unchanged frozen crates.
///
/// This type supplies storage access, not import scheduling. Its extra import methods expose
/// declaration lists and the proc-macro export rule for the active builder.
pub(crate) struct BuildResolutionEnv<'a> {
    baseline: &'a DefMapReadTxn<'a>,
    completed: &'a CompletedCrateStore,
    state: &'a CrateBuilder,
}

impl<'a> BuildResolutionEnv<'a> {
    pub(crate) fn new(
        baseline: &'a DefMapReadTxn<'a>,
        completed: &'a CompletedCrateStore,
        state: &'a CrateBuilder,
    ) -> Self {
        Self {
            baseline,
            completed,
            state,
        }
    }

    /// A selected package never falls back to its previous snapshot, even if a crate slot has
    /// not been filled yet. Dependency order makes all reachable external crates complete.
    fn frozen_crate(&self, krate: CrateRef) -> Result<Option<&CrateData>, PackageStoreError> {
        if self.completed.contains_package(krate.package) {
            return Ok(self.completed.crate_data(krate));
        }
        self.baseline.crate_data(krate)
    }

    fn active_crate(&self, krate: CrateRef) -> Option<&CrateBuilder> {
        (krate == self.state.crate_ref).then_some(self.state)
    }
}

impl NameResolutionEnv for BuildResolutionEnv<'_> {
    type Error = PackageStoreError;

    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, Self::Error> {
        if let Some(crate_ref) = module_ref.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
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
            .frozen_crate(crate_ref)?
            .map(CrateData::def_map)
            .and_then(|def_map| def_map.module(module_ref.module))
            .map(ModuleData::resolution_view))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<NameBindingsView<'a>>, Self::Error> {
        let Some(crate_ref) = module_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        if crate_ref == self.state.crate_ref {
            return Ok(self
                .state
                .def_map_builder
                .module_scope(module_ref.module)
                .and_then(|scope| scope.entry(name)));
        }

        Ok(self
            .frozen_crate(crate_ref)?
            .and_then(|krate| krate.def_map().module(module_ref.module))
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_view()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, NameBindingsView<'a>)>, Self::Error> {
        let Some(crate_ref) = module_ref.origin.as_crate_ref() else {
            return Ok(Vec::new());
        };
        if crate_ref == self.state.crate_ref {
            return Ok(self
                .state
                .def_map_builder
                .module_scope(module_ref.module)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }

        Ok(self
            .frozen_crate(crate_ref)?
            .and_then(|krate| krate.def_map().module(module_ref.module))
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
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state.def_map_builder.local_def(local_def_ref.local_def));
        }

        let Some(crate_ref) = local_def_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .frozen_crate(crate_ref)?
            .map(CrateData::def_map)
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        if let Some(crate_ref) = variant_ref.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant(variant_ref.local_enum_variant));
        }

        let Some(crate_ref) = variant_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .frozen_crate(crate_ref)?
            .map(CrateData::def_map)
            .and_then(|def_map| def_map.local_enum_variant(variant_ref.local_enum_variant)))
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect());
        }

        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(def_map) = self.frozen_crate(crate_ref)?.map(CrateData::def_map)
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
            && let Some(state) = self.active_crate(crate_ref)
        {
            state.def_map_builder.macro_definition(def_ref.local_def)
        } else {
            let Some(crate_ref) = def_ref.origin.as_crate_ref() else {
                return Ok(None);
            };
            self.frozen_crate(crate_ref)?
                .map(CrateData::def_map)
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
        if let Some(state) = self.active_crate(crate_ref) {
            return Ok(state.extern_prelude.resolve(name));
        }

        Ok(self
            .frozen_crate(crate_ref)?
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        if let Some(state) = self.active_crate(crate_ref) {
            return Ok(state.prelude);
        }

        Ok(self
            .frozen_crate(crate_ref)?
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        let module = if let Some(state) = self.active_crate(crate_ref) {
            Some(state.root_module)
        } else {
            self.frozen_crate(crate_ref)?
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
        let state = self.state;
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
        result
    }

    fn import_data(&self, import: ImportRef) -> &ImportData {
        assert_eq!(
            import.origin.as_crate_ref(),
            Some(self.state.crate_ref),
            "import belongs to the active crate"
        );
        &self.state.def_map_builder.imports()[import.import.0]
    }

    fn allow_public_binding(
        &self,
        module: ModuleRef,
        namespace: Namespace,
        binding: &NameBinding,
    ) -> bool {
        self.state
            .allow_public_binding(module.module, namespace, binding)
    }
}
