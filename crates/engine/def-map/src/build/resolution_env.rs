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
    CrateRef, DefId, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleId, ModuleRef,
};
use rg_source::ReadError;
use rg_text::Name;

use super::{
    crates::CrateBuilder,
    imports::{ImportCollectionEnv, ModuleImportInput},
};
use crate::{
    CrateResolutionEnv, ImportData, LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry,
    MacroDefinitionEnv, MacroDefinitionView, ModuleBuilder, ModuleResolutionView, NameBinding,
    NameBindingsView, NameResolutionEnv, Namespace,
};

/// Reads the active builder alongside completed and unchanged frozen crates.
///
/// This type supplies storage access, not import scheduling. Its extra import methods expose
/// declaration lists and the proc-macro export rule for the active builder.
pub(crate) struct BuildResolutionEnv<'a, E> {
    dependencies: &'a E,
    state: &'a CrateBuilder,
}

impl<'a, E> BuildResolutionEnv<'a, E> {
    pub(crate) fn new(dependencies: &'a E, state: &'a CrateBuilder) -> Self {
        Self {
            dependencies,
            state,
        }
    }

    fn active_crate(&self, krate: CrateRef) -> Option<&CrateBuilder> {
        (krate == self.state.crate_ref).then_some(self.state)
    }
}

impl<E: NameResolutionEnv> NameResolutionEnv for BuildResolutionEnv<'_, E> {
    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, ReadError> {
        if let Some(crate_ref) = module_ref.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .module(module_ref.module)
                .map(ModuleBuilder::resolution_view));
        }

        self.dependencies.module_metadata(module_ref)
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<NameBindingsView<'a>>, ReadError> {
        if module_ref.origin.as_crate_ref() == Some(self.state.crate_ref) {
            return Ok(self
                .state
                .def_map_builder
                .module_scope(module_ref.module)
                .and_then(|scope| scope.entry(name)));
        }
        self.dependencies.module_scope_entry(module_ref, name)
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, NameBindingsView<'a>)>, ReadError> {
        if module_ref.origin.as_crate_ref() == Some(self.state.crate_ref) {
            return Ok(self
                .state
                .def_map_builder
                .module_scope(module_ref.module)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }
        self.dependencies.module_scope_entries(module_ref)
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, ReadError> {
        if let Some(crate_ref) = local_def_ref.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state.def_map_builder.local_def(local_def_ref.local_def));
        }

        self.dependencies.local_def_data(local_def_ref)
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, ReadError> {
        if let Some(crate_ref) = variant_ref.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant(variant_ref.local_enum_variant));
        }

        self.dependencies.local_enum_variant_data(variant_ref)
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, ReadError> {
        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(state) = self.active_crate(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect());
        }

        self.dependencies
            .local_enum_variant_entries_for_enum(enum_def)
    }
}

impl<E: MacroDefinitionEnv> MacroDefinitionEnv for BuildResolutionEnv<'_, E> {
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, ReadError> {
        let DefId::Local(def_ref) = def else {
            return Ok(None);
        };
        if def_ref.origin.as_crate_ref() != Some(self.state.crate_ref) {
            return self.dependencies.macro_definition_view(def);
        }
        let Some(local_def) = self.state.def_map_builder.local_def(def_ref.local_def) else {
            return Ok(None);
        };
        Ok(self
            .state
            .def_map_builder
            .macro_definition(def_ref.local_def)
            .and_then(|data| MacroDefinitionView::new(def_ref, local_def, data)))
    }
}

impl<E: CrateResolutionEnv> CrateResolutionEnv for BuildResolutionEnv<'_, E> {
    fn extern_root(&self, crate_ref: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        if let Some(state) = self.active_crate(crate_ref) {
            return Ok(state.extern_prelude.resolve(name));
        }
        self.dependencies.extern_root(crate_ref, name)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        if let Some(state) = self.active_crate(crate_ref) {
            return Ok(state.prelude);
        }
        self.dependencies.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        if let Some(state) = self.active_crate(crate_ref) {
            return Ok(Some(ModuleRef::krate(crate_ref, state.root_module)));
        }
        self.dependencies.root_module(crate_ref)
    }
}

impl<E> ImportCollectionEnv for BuildResolutionEnv<'_, E>
where
    E: CrateResolutionEnv,
{
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
