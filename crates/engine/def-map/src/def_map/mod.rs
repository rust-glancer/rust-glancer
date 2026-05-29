use std::{collections::HashMap, sync::Arc};

use rg_ir_model::{
    BodyRef, DefMapRef, ImportId, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
    ModuleRef, TargetRef,
    hir::source::{GeneratedSourceData, GeneratedSourceId},
};
use rg_text::Name;

use crate::{ImportData, LocalDefData, LocalImplData, MacroDefinitionData, ModuleData};

use self::{def_map_data::DefMapData, target_data::TargetData};

mod def_map_data;
mod target_data;

/// Frozen namespace map for one analyzed scope.
///
/// There might be several defmaps per target:
/// the root defmap represents the semantic layer, but also
/// each body function has its own defmap that tracks the body-local items.
/// While functions are not really modules, they work similarly, and we model
/// them as if each scope is a module.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct DefMap {
    /// Whether this defmap corresponds to the target root.
    is_root: bool,
    /// Ref to this defmap, which can be used to emit correct
    /// refs.
    own_ref: DefMapRef,
    /// Shared data on the target.
    target_data: Arc<TargetData>,
    /// Actual defmap layout for the corresponding scope.
    data: DefMapData,
}

impl rg_memsize::MemorySize for DefMap {
    fn record_memory_children(&self, recorder: &mut rg_memsize::MemoryRecorder) {
        if self.is_root {
            recorder.scope("target_data", |recorder| {
                rg_memsize::MemorySize::record_memory_children(&self.target_data, recorder);
            });
        }

        recorder.scope("data", |recorder| {
            rg_memsize::MemorySize::record_memory_children(&self.data, recorder);
        });
    }
}

impl DefMap {
    pub(crate) fn empty(target: TargetRef) -> Self {
        let target_data = Arc::new(TargetData {
            target,
            root_module: None,
            extern_prelude: HashMap::default(),
            prelude: None,
        });

        Self {
            is_root: true,
            own_ref: DefMapRef::Target(target),
            target_data,
            data: DefMapData::default(),
        }
    }

    /// Creates a derived defmap that can be used for the body function scope.
    pub fn child(&self, body_ref: BodyRef) -> Self {
        Self {
            is_root: false,
            own_ref: DefMapRef::Body(body_ref),
            target_data: self.target_data.clone(),
            data: DefMapData::default(),
        }
    }

    /// Returns the root module of this target, if the map has been populated.
    // TODO: Should probably be moved to target data so that it's not confusing
    pub(crate) fn root_module(&self) -> Option<ModuleId> {
        self.target_data.root_module
    }

    /// Returns the external root names visible from this target.
    // TODO: Should probably be moved to target data so that it's not confusing
    pub(crate) fn extern_prelude(&self) -> &HashMap<Name, ModuleRef> {
        &self.target_data.extern_prelude
    }

    /// Returns the standard prelude module visible from this target, if it was discovered.
    // TODO: Should probably be moved to target data so that it's not confusing
    pub(crate) fn prelude(&self) -> Option<ModuleRef> {
        self.target_data.prelude
    }

    /// Returns all modules in stable module-id order.
    pub fn modules(&self) -> &[ModuleData] {
        self.data.modules.as_slice()
    }

    /// Returns refs for all the modules in stable module-id order.
    pub fn module_refs(&self) -> impl Iterator<Item = ModuleRef> {
        (0..self.data.modules.len()).map(|id| ModuleRef {
            origin: self.own_ref,
            module: ModuleId(id),
        })
    }

    /// Returns module data by id.
    pub fn module(&self, module_id: ModuleId) -> Option<&ModuleData> {
        self.data.modules.get(module_id)
    }

    pub(crate) fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleData> {
        self.data.modules.get_mut(module_id)
    }

    pub(crate) fn module_count(&self) -> usize {
        self.data.modules.len()
    }

    /// Returns local definition data by id.
    pub fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.data.local_defs.get(local_def)
    }

    /// Returns all local definitions in stable local-def-id order.
    pub fn local_defs(&self) -> &[LocalDefData] {
        self.data.local_defs.as_slice()
    }

    pub fn local_def_refs(&self) -> impl Iterator<Item = LocalDefRef> {
        (0..self.data.local_defs.len()).map(|id| LocalDefRef {
            origin: self.own_ref,
            local_def: LocalDefId(id),
        })
    }

    /// Returns a declarative macro payload by its local definition id.
    pub(crate) fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
        self.data.macro_definitions.get(&local_def)
    }

    /// Returns impl block data by id.
    pub fn local_impl(&self, local_impl: LocalImplId) -> Option<&LocalImplData> {
        self.data.local_impls.get(local_impl)
    }

    /// Returns all impl blocks in stable local-impl-id order.
    pub fn local_impls(&self) -> &[LocalImplData] {
        self.data.local_impls.as_slice()
    }

    pub fn lodal_impl_refs(&self) -> impl Iterator<Item = LocalImplRef> {
        (0..self.data.local_impls.len()).map(|id| LocalImplRef {
            origin: self.own_ref,
            local_impl: LocalImplId(id),
        })
    }

    /// Returns all imports in stable import-id order.
    pub fn imports(&self) -> &[ImportData] {
        self.data.imports.as_slice()
    }

    /// Returns one retained generated source by id.
    pub fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&GeneratedSourceData> {
        self.data.generated_sources.get(generated_source)
    }

    /// Returns all retained generated sources in stable generated-source-id order.
    pub fn generated_sources(&self) -> &[GeneratedSourceData] {
        self.data.generated_sources.as_slice()
    }

    pub(crate) fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.data.imports.iter_with_ids()
    }

    pub(crate) fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        self.data.modules.alloc(module)
    }

    pub(crate) fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.data.local_defs.alloc(local_def)
    }

    pub(crate) fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.data
            .macro_definitions
            .insert(local_def, macro_definition);
    }

    pub(crate) fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.data.local_impls.alloc(local_impl)
    }

    pub(crate) fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.data.imports.alloc(import)
    }

    pub(crate) fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.data.generated_sources.alloc(generated_source)
    }

    pub(crate) fn set_root_module(&mut self, root_module: ModuleId) {
        self.target_data_mut().root_module = Some(root_module);
    }

    pub(crate) fn set_extern_prelude(&mut self, extern_prelude: HashMap<Name, ModuleRef>) {
        self.target_data_mut().extern_prelude = extern_prelude;
    }

    pub(crate) fn set_prelude(&mut self, prelude: Option<ModuleRef>) {
        self.target_data_mut().prelude = prelude;
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.target_data_mut().shrink_to_fit();
        self.data.shrink_to_fit();
    }

    fn target_data_mut(&mut self) -> &mut TargetData {
        Arc::make_mut(&mut self.target_data)
    }
}
