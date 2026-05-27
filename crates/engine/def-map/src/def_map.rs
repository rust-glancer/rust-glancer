use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{
    ImportId, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId, ModuleRef, TargetRef,
};
use rg_text::Name;

use super::{GeneratedSourceData, GeneratedSourceId, ImportData};
use crate::{LocalDefData, LocalImplData, MacroDefinitionData, ModuleData};

/// Frozen namespace map for one analyzed target.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct DefMap {
    // Target this defmap corresponds to
    target: TargetRef,

    root_module: Option<ModuleId>,
    // Currently means “implicit roots visible to this target,” including sibling lib roots
    extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this target, if sysroot sources are available.
    prelude: Option<ModuleRef>,
    modules: Arena<ModuleId, ModuleData>,
    local_defs: Arena<LocalDefId, LocalDefData>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    local_impls: Arena<LocalImplId, LocalImplData>,
    imports: Arena<ImportId, ImportData>,
    generated_sources: Arena<GeneratedSourceId, GeneratedSourceData>,
}

impl DefMap {
    pub(crate) fn empty(target: TargetRef) -> Self {
        Self {
            target,
            root_module: None,
            extern_prelude: HashMap::default(),
            prelude: None,
            modules: Arena::default(),
            local_defs: Arena::default(),
            macro_definitions: HashMap::default(),
            local_impls: Arena::default(),
            imports: Arena::default(),
            generated_sources: Arena::default(),
        }
    }

    /// Returns the root module of this target, if the map has been populated.
    pub(crate) fn root_module(&self) -> Option<ModuleId> {
        self.root_module
    }

    /// Returns the external root names visible from this target.
    pub(crate) fn extern_prelude(&self) -> &HashMap<Name, ModuleRef> {
        &self.extern_prelude
    }

    /// Returns the standard prelude module visible from this target, if it was discovered.
    pub(crate) fn prelude(&self) -> Option<ModuleRef> {
        self.prelude
    }

    /// Returns all modules in stable module-id order.
    pub fn modules(&self) -> &[ModuleData] {
        self.modules.as_slice()
    }

    /// Returns refs for all the modules in stable module-id order.
    pub fn module_refs(&self) -> impl Iterator<Item = ModuleRef> {
        (0..self.modules.len()).map(|id| ModuleRef {
            target: self.target,
            module: ModuleId(id),
        })
    }

    /// Returns module data by id.
    pub fn module(&self, module_id: ModuleId) -> Option<&ModuleData> {
        self.modules.get(module_id)
    }

    pub(crate) fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleData> {
        self.modules.get_mut(module_id)
    }

    pub(crate) fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Returns local definition data by id.
    pub fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.local_defs.get(local_def)
    }

    /// Returns all local definitions in stable local-def-id order.
    pub fn local_defs(&self) -> &[LocalDefData] {
        self.local_defs.as_slice()
    }

    pub fn local_def_refs(&self) -> impl Iterator<Item = LocalDefRef> {
        (0..self.local_defs.len()).map(|id| LocalDefRef {
            target: self.target,
            local_def: LocalDefId(id),
        })
    }

    /// Returns a declarative macro payload by its local definition id.
    pub(crate) fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
        self.macro_definitions.get(&local_def)
    }

    /// Returns impl block data by id.
    pub fn local_impl(&self, local_impl: LocalImplId) -> Option<&LocalImplData> {
        self.local_impls.get(local_impl)
    }

    /// Returns all impl blocks in stable local-impl-id order.
    pub fn local_impls(&self) -> &[LocalImplData] {
        self.local_impls.as_slice()
    }

    pub fn lodal_impl_refs(&self) -> impl Iterator<Item = LocalImplRef> {
        (0..self.local_impls.len()).map(|id| LocalImplRef {
            target: self.target,
            local_impl: LocalImplId(id),
        })
    }

    /// Returns all imports in stable import-id order.
    pub fn imports(&self) -> &[ImportData] {
        self.imports.as_slice()
    }

    /// Returns one retained generated source by id.
    pub fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&GeneratedSourceData> {
        self.generated_sources.get(generated_source)
    }

    /// Returns all retained generated sources in stable generated-source-id order.
    pub fn generated_sources(&self) -> &[GeneratedSourceData] {
        self.generated_sources.as_slice()
    }

    pub(crate) fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.imports.iter_with_ids()
    }

    pub(crate) fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        self.modules.alloc(module)
    }

    pub(crate) fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.local_defs.alloc(local_def)
    }

    pub(crate) fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.macro_definitions.insert(local_def, macro_definition);
    }

    pub(crate) fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.local_impls.alloc(local_impl)
    }

    pub(crate) fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.imports.alloc(import)
    }

    pub(crate) fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.generated_sources.alloc(generated_source)
    }

    pub(crate) fn set_root_module(&mut self, root_module: ModuleId) {
        self.root_module = Some(root_module);
    }

    pub(crate) fn set_extern_prelude(&mut self, extern_prelude: HashMap<Name, ModuleRef>) {
        self.extern_prelude = extern_prelude;
    }

    pub(crate) fn set_prelude(&mut self, prelude: Option<ModuleRef>) {
        self.prelude = prelude;
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.extern_prelude.shrink_to_fit();
        self.modules.shrink_to_fit();
        for module in self.modules.iter_mut() {
            module.shrink_to_fit();
        }
        self.local_defs.shrink_to_fit();
        for local_def in self.local_defs.iter_mut() {
            local_def.shrink_to_fit();
        }
        self.macro_definitions.shrink_to_fit();
        for macro_definition in self.macro_definitions.values_mut() {
            macro_definition.shrink_to_fit();
        }
        self.local_impls.shrink_to_fit();
        self.imports.shrink_to_fit();
        for import in self.imports.iter_mut() {
            import.shrink_to_fit();
        }
        self.generated_sources.shrink_to_fit();
        for generated_source in self.generated_sources.iter_mut() {
            generated_source.shrink_to_fit();
        }
    }
}
