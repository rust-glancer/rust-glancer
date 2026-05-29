use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{
    ImportId, LocalDefId, LocalImplId, ModuleId,
    hir::source::{GeneratedSourceData, GeneratedSourceId},
};

use crate::{ImportData, LocalDefData, LocalImplData, MacroDefinitionData, ModuleData};

#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Eq,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub(super) struct DefMapData {
    pub(super) modules: Arena<ModuleId, ModuleData>,
    pub(super) local_defs: Arena<LocalDefId, LocalDefData>,
    pub(super) macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    pub(super) local_impls: Arena<LocalImplId, LocalImplData>,
    pub(super) imports: Arena<ImportId, ImportData>,
    pub(super) generated_sources: Arena<GeneratedSourceId, GeneratedSourceData>,
}

impl DefMapData {
    pub(super) fn shrink_to_fit(&mut self) {
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
