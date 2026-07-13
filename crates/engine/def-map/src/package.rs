use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{CrateId, ModuleId, ModuleRef};
use rg_parse::CargoTargetId;
use rg_std::{MemorySize, Shrink};
use rg_text::{Name, RustEdition};
use wincode::{SchemaRead, SchemaWrite};

use crate::map::DefMap;

/// Definition and resolution state for one semantic crate.
///
/// Cargo targets are source/project inputs. The semantic engine assigns a package-local `CrateId`
/// while retaining the originating target id here for the few operations that need to return to
/// parsed source data.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateData {
    cargo_target: CargoTargetId,
    name: String,
    root_module: Option<ModuleId>,
    // Implicit roots visible to this crate, including sibling lib roots.
    extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this crate, if sysroot sources are available.
    prelude: Option<ModuleRef>,
    def_map: DefMap,
}

impl CrateData {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cargo_target: CargoTargetId,
        name: String,
        root_module: Option<ModuleId>,
        extern_prelude: HashMap<Name, ModuleRef>,
        prelude: Option<ModuleRef>,
        def_map: DefMap,
    ) -> Self {
        Self {
            cargo_target,
            name,
            root_module,
            extern_prelude,
            prelude,
            def_map,
        }
    }

    /// Returns the parsed Cargo target that produced this semantic crate.
    pub fn cargo_target(&self) -> CargoTargetId {
        self.cargo_target
    }

    /// Returns the crate name used by Rust path resolution.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the root module of this crate, if the map has been populated.
    // TODO: The root should become required now that def-map construction has a builder phase.
    pub fn root_module(&self) -> Option<ModuleId> {
        self.root_module
    }

    /// Returns the external root names visible from this crate.
    pub fn extern_prelude(&self) -> &HashMap<Name, ModuleRef> {
        &self.extern_prelude
    }

    /// Returns the standard prelude module visible from this crate, if it was discovered.
    pub fn prelude(&self) -> Option<ModuleRef> {
        self.prelude
    }

    pub fn def_map(&self) -> &DefMap {
        &self.def_map
    }
}

/// Def maps for all semantic crates inside one parsed package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageDefMaps {
    pub(crate) name: String,
    pub(crate) edition: RustEdition,
    pub(crate) crates: Arena<CrateId, CrateData>,
}

impl PackageDefMaps {
    pub fn new(name: String, edition: RustEdition, crates: Vec<CrateData>) -> Self {
        Self {
            name,
            edition,
            crates: Arena::from_vec(crates),
        }
    }

    /// Returns the Cargo package name this def-map package belongs to.
    pub fn package_name(&self) -> &str {
        &self.name
    }

    /// Returns the Rust edition shared by all crates in this Cargo package.
    pub fn edition(&self) -> RustEdition {
        self.edition
    }

    /// Returns all semantic crates in crate-id order.
    pub fn crates(&self) -> &[CrateData] {
        self.crates.as_slice()
    }

    /// Returns one semantic crate by its package-local identity.
    pub fn crate_data(&self, crate_id: CrateId) -> Option<&CrateData> {
        self.crates.get(crate_id)
    }

    /// Finds the semantic crate produced from one parsed Cargo target.
    pub fn crate_for_cargo_target(&self, cargo_target: CargoTargetId) -> Option<CrateId> {
        self.crates
            .iter_with_ids()
            .find_map(|(crate_id, data)| (data.cargo_target() == cargo_target).then_some(crate_id))
    }

    pub fn def_map(&self, crate_id: CrateId) -> Option<&DefMap> {
        self.crate_data(crate_id).map(CrateData::def_map)
    }
}
