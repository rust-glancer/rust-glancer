use rg_ir_model::{
    BodyRef, DefMapRef, ImportId, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
    ModuleRef, TargetRef,
    hir::source::{GeneratedSourceData, GeneratedSourceId},
};

use crate::{ImportData, LocalDefData, LocalImplData, MacroDefinitionData, ModuleData};

use self::def_map_data::DefMapData;

mod def_map_data;
pub(crate) mod target_data;

pub(crate) use self::target_data::TargetData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefMapBuilder {
    def_map: DefMap,
}

impl DefMapBuilder {
    pub fn new(target: TargetRef) -> Self {
        Self {
            def_map: DefMap::target(target),
        }
    }

    pub fn new_body(body_ref: BodyRef) -> Self {
        Self {
            def_map: DefMap::body(body_ref),
        }
    }

    /// DefMap is first build, and then populated.
    /// This method provides a method to access the defmap directly, but it's the caller's
    /// responsibility to make sure that the defmap has been populated already.
    // TODO: Add `collected` flag to forbid adding more things and only allow mutating
    // existing ones.
    pub fn as_incomplete_def_map(&self) -> &DefMap {
        &self.def_map
    }

    pub fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleData> {
        self.def_map.data.modules.get_mut(module_id)
    }

    pub fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        self.def_map.data.modules.alloc(module)
    }

    pub fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.def_map.data.local_defs.alloc(local_def)
    }

    pub fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.def_map
            .data
            .macro_definitions
            .insert(local_def, macro_definition);
    }

    pub fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.def_map.data.local_impls.alloc(local_impl)
    }

    pub fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.def_map.data.imports.alloc(import)
    }

    pub fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.def_map.data.generated_sources.alloc(generated_source)
    }

    pub fn build(self) -> DefMap {
        self.def_map
    }
}

/// Frozen namespace map for one analyzed scope.
///
/// There might be several defmaps per target:
/// the root defmap represents the semantic layer, but also
/// each body function has its own defmap that tracks the body-local items.
/// While functions are not really modules, they work similarly, and we model
/// them as if each scope is a module.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct DefMap {
    /// Ref to this defmap, which can be used to emit correct refs.
    own_ref: DefMapRef,
    /// Actual defmap layout for the corresponding scope.
    data: DefMapData,
}

impl DefMap {
    fn target(target: TargetRef) -> Self {
        Self {
            own_ref: DefMapRef::Target(target),
            data: DefMapData::default(),
        }
    }

    fn body(body_ref: BodyRef) -> Self {
        Self {
            own_ref: DefMapRef::Body(body_ref),
            data: DefMapData::default(),
        }
    }

    pub fn own_ref(&self) -> DefMapRef {
        self.own_ref
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
    pub fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
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

    pub fn local_impl_refs(&self) -> impl Iterator<Item = LocalImplRef> {
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

    pub fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.data.imports.iter_with_ids()
    }

    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
    }
}

impl rg_memsize::MemorySize for DefMap {
    fn record_memory_children(&self, recorder: &mut rg_memsize::MemoryRecorder) {
        recorder.scope("data", |recorder| {
            rg_memsize::MemorySize::record_memory_children(&self.data, recorder);
        });
    }
}
