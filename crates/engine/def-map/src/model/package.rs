use rg_arena::Arena;
use rg_parse::TargetId;

use crate::DefMap;

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
    pub(crate) targets: Arena<TargetId, DefMap>,
}

impl PackageDefMaps {
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

    /// Returns one target def map by target id.
    pub fn def_map(&self, target_id: TargetId) -> Option<&DefMap> {
        self.targets.get(target_id)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.target_names.shrink_to_fit();
        for target_name in self.target_names.iter_mut() {
            target_name.shrink_to_fit();
        }
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }
}
