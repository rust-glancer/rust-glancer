use std::collections::HashMap;

use rg_ir_model::{ModuleId, ModuleRef, TargetRef};
use rg_text::Name;

#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub(super) struct TargetData {
    // Target this defmap corresponds to
    pub(super) target: TargetRef,

    pub(super) root_module: Option<ModuleId>,
    // Currently means “implicit roots visible to this target,” including sibling lib roots
    pub(super) extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this target, if sysroot sources are available.
    pub(super) prelude: Option<ModuleRef>,
}

impl TargetData {
    pub(super) fn shrink_to_fit(&mut self) {
        self.extern_prelude.shrink_to_fit();
    }
}
