use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{CrateId, ModuleId, ModuleRef};
use rg_parse::CargoTargetId;
use rg_std::{MemorySize, Shrink};
use rg_text::{Name, RustEdition};
use rg_workspace::TargetKind;
use wincode::{SchemaRead, SchemaWrite};

use crate::map::DefMap;

/// Definition and resolution state for one semantic crate.
///
/// Cargo targets are source/project inputs. The semantic engine assigns a package-local `CrateId`
/// while retaining the originating target id and kind here. The id lets source-facing operations
/// return to parsed data; the kind preserves language boundaries such as the difference between a
/// proc-macro export and its host-side implementation crate.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateData {
    cargo_target: CargoTargetId,
    target_kind: TargetKind,
    name: String,
    root_module: Option<ModuleId>,
    // Crate-wide extern roots from Cargo dependencies and root `extern crate` declarations.
    extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this crate, if sysroot sources are available.
    prelude: Option<ModuleRef>,
    def_map: DefMap,
}

impl CrateData {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cargo_target: CargoTargetId,
        target_kind: TargetKind,
        name: String,
        root_module: Option<ModuleId>,
        extern_prelude: HashMap<Name, ModuleRef>,
        prelude: Option<ModuleRef>,
        def_map: DefMap,
    ) -> Self {
        Self {
            cargo_target,
            target_kind,
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

    /// Returns the Cargo target role whose language rules apply to this semantic crate.
    pub fn target_kind(&self) -> &TargetKind {
        &self.target_kind
    }

    /// Whether this crate exposes proc-macro identities across its crate boundary.
    pub fn is_proc_macro(&self) -> bool {
        self.target_kind.is_proc_macro()
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

/// Frozen def maps and bounded build diagnostics for one parsed package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageDefMaps {
    pub(crate) name: String,
    pub(crate) edition: RustEdition,
    pub(crate) crates: Arena<CrateId, CrateData>,
    macro_expansion_limits: Vec<MacroExpansionLimitReport>,
}

impl PackageDefMaps {
    pub fn new(
        name: String,
        edition: RustEdition,
        crates: Vec<CrateData>,
        macro_expansion_limits: Vec<MacroExpansionLimitReport>,
    ) -> Self {
        Self {
            name,
            edition,
            crates: Arena::from_vec(crates),
            macro_expansion_limits,
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

    /// Returns bounded diagnostics retained when this package hit the global macro pass limit.
    pub fn macro_expansion_limits(&self) -> &[MacroExpansionLimitReport] {
        &self.macro_expansion_limits
    }
}

/// Bounded summary of calls left retryable in one crate when the macro guard fired.
///
/// The summary groups calls by their rendered macro path and retains one ancestry example instead
/// of tokens or a complete expansion graph. A pathological expansion therefore stays cheap to
/// persist in package artifacts and copy into reports.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroExpansionLimitReport {
    /// Cargo package containing the affected semantic crate.
    pub package_name: String,
    /// Semantic crate whose worklist reached the guard.
    pub crate_name: String,
    /// Represented calls grouped by rendered macro identity.
    pub groups: Vec<MacroExpansionLimitGroup>,
    /// Calls not represented after the shared rendered-group budget was exhausted.
    pub omitted_call_count: usize,
}

/// Calls with one rendered macro identity that were skipped by the expansion limit.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroExpansionLimitGroup {
    /// Full written path when available, otherwise the final callee name or `<unknown>`.
    pub macro_name: String,
    /// Total calls represented by this group.
    pub skipped_call_count: usize,
    /// Represented calls written directly in source.
    pub source_call_count: usize,
    /// Represented calls produced by another macro expansion.
    pub generated_call_count: usize,
    /// One source-to-leaf ancestry example, such as `entry -> recurse -> recurse`.
    pub example_chain: Vec<String>,
    /// Whether the ancestry example stopped before reaching a valid source call.
    pub chain_truncated: bool,
}
