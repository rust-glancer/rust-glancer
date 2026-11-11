use std::collections::HashMap;

use rg_ir_model::{CrateRef, ModuleId, ModuleRef};
use rg_parse::CargoTargetId;
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
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
    // Const arithmetic must use the compilation target even when it differs from the host.
    target_pointer_width: Option<u32>,
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
        target_pointer_width: Option<u32>,
        name: String,
        root_module: Option<ModuleId>,
        extern_prelude: HashMap<Name, ModuleRef>,
        prelude: Option<ModuleRef>,
        def_map: DefMap,
    ) -> Self {
        Self {
            cargo_target,
            target_kind,
            target_pointer_width,
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

    pub fn target_pointer_width(&self) -> Option<u32> {
        self.target_pointer_width
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

    /// Returns crate dependencies whose ordinary declarations participate in item lookup.
    ///
    /// Names are irrelevant to this traversal. Keeping only crate identities in the persisted
    /// directory lets type queries walk the dependency graph without decoding module scopes.
    pub(crate) fn item_lookup_dependencies(&self) -> UniqueVec<CrateRef> {
        let mut dependencies = self
            .extern_prelude
            .values()
            .filter_map(|module| module.origin.as_crate_ref())
            .collect::<Vec<_>>();
        if let Some(prelude) = self.prelude
            && let Some(crate_ref) = prelude.origin.as_crate_ref()
        {
            dependencies.push(crate_ref);
        }
        // Hash-map discovery order must not leak into persisted routing or visibility traversal.
        // Sort first, then let the ordered-set representation remove duplicate crate identities.
        dependencies.sort_by_key(|crate_ref| (crate_ref.package.0, crate_ref.crate_id.0));
        dependencies.into_iter().collect()
    }
}

/// Bounded summary of invocations stopped by the macro recursion limit in one crate.
///
/// The summary groups calls by their rendered macro path and retains one ancestry example instead
/// of tokens or a complete expansion graph. A pathological expansion therefore stays cheap to
/// retain for diagnostics.
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
