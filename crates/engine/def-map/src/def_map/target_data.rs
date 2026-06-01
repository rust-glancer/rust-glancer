use std::collections::HashMap;

use rg_ir_model::{ModuleId, ModuleRef};
use rg_text::Name;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct TargetData {
    pub(super) root_module: Option<ModuleId>,
    // Implicit roots visible to this target, including sibling lib roots.
    pub(super) extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this target, if sysroot sources are available.
    pub(super) prelude: Option<ModuleRef>,
}

impl TargetData {
    pub(crate) fn new(
        root_module: Option<ModuleId>,
        extern_prelude: HashMap<Name, ModuleRef>,
        prelude: Option<ModuleRef>,
    ) -> Self {
        Self {
            root_module,
            extern_prelude,
            prelude,
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.extern_prelude.shrink_to_fit();
    }

    /// Returns the root module of this target, if the map has been populated.
    // TODO: Also I guess it should not be an option given that we have builder now.
    pub fn root_module(&self) -> Option<ModuleId> {
        self.root_module
    }

    /// Returns the external root names visible from this target.
    pub fn extern_prelude(&self) -> &HashMap<Name, ModuleRef> {
        &self.extern_prelude
    }

    /// Returns the standard prelude module visible from this target, if it was discovered.
    pub fn prelude(&self) -> Option<ModuleRef> {
        self.prelude
    }
}
