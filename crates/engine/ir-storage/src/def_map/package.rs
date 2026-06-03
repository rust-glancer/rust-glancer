use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{ModuleId, ModuleRef};
use rg_parse::TargetId;
use rg_text::Name;

use super::store::DefMap;

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
    pub fn new(
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

/// Def maps for all targets inside one parsed package.
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
pub struct PackageDefMaps {
    pub(crate) name: String,
    pub(crate) target_names: Arena<TargetId, String>,
    pub(crate) target_data: Arena<TargetId, TargetData>,
    pub(crate) targets: Arena<TargetId, DefMap>,
}

impl PackageDefMaps {
    pub fn new(
        name: String,
        target_names: Vec<String>,
        target_data: Vec<TargetData>,
        targets: Vec<DefMap>,
    ) -> Self {
        debug_assert_eq!(
            target_names.len(),
            target_data.len(),
            "target names and target data should describe the same targets",
        );
        debug_assert_eq!(
            target_data.len(),
            targets.len(),
            "target data and def maps should describe the same targets",
        );

        Self {
            name,
            target_names: Arena::from_vec(target_names),
            target_data: Arena::from_vec(target_data),
            targets: Arena::from_vec(targets),
        }
    }

    /// Returns the Cargo package name this def-map package belongs to.
    pub fn package_name(&self) -> &str {
        &self.name
    }

    /// Returns the crate name for one target, if that target exists.
    pub fn target_name(&self, target_id: TargetId) -> Option<&str> {
        self.target_names.get(target_id).map(String::as_str)
    }

    /// Returns all target def maps in target-id order.
    pub fn def_maps(&self) -> &[DefMap] {
        self.targets.as_slice()
    }

    /// Returns target-wide resolution facts for one target.
    pub fn target_data(&self, target_id: TargetId) -> Option<&TargetData> {
        self.target_data.get(target_id)
    }

    /// Returns one target def map by target id.
    pub fn def_map(&self, target_id: TargetId) -> Option<&DefMap> {
        self.targets.get(target_id)
    }

    pub fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.target_names.shrink_to_fit();
        for target_name in self.target_names.iter_mut() {
            target_name.shrink_to_fit();
        }
        self.target_data.shrink_to_fit();
        for target_data in self.target_data.iter_mut() {
            target_data.shrink_to_fit();
        }
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }
}
