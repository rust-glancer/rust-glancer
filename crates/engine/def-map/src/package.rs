use std::collections::HashMap;

use rg_ir_model::{CrateRef, ModuleId, ModuleRef};
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use rg_workspace::TargetKind;
use wincode::{SchemaRead, SchemaWrite};

use crate::map::DefMap;

/// Completed declaration collection, before publication into independently readable outputs.
///
/// Builders use the complete map to finish resolution. Target publication then separates the
/// crate-wide facts in [`CrateMetadata`] from the namespace records in [`DefMap`].
///
/// The crate's identity is its configured target's `CrateRef`. The target kind preserves language
/// boundaries such as the difference between a proc-macro export and its host-side implementation.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub struct CrateData {
    metadata: CrateMetadata,
    def_map: DefMap,
}

impl CrateData {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        target_kind: TargetKind,
        target_pointer_width: Option<u32>,
        name: String,
        root_module: ModuleId,
        extern_prelude: HashMap<Name, ModuleRef>,
        prelude: Option<ModuleRef>,
        def_map: DefMap,
    ) -> Self {
        Self {
            metadata: CrateMetadata {
                target_kind,
                target_pointer_width,
                name,
                root_module,
                extern_prelude,
                prelude,
            },
            def_map,
        }
    }

    pub fn metadata(&self) -> &CrateMetadata {
        &self.metadata
    }

    pub fn def_map(&self) -> &DefMap {
        &self.def_map
    }

    pub fn into_parts(self) -> (CrateMetadata, DefMap) {
        (self.metadata, self.def_map)
    }
}

/// Facts shared by the crate's modules, such as its name, external roots, and standard prelude.
/// Keeping these separate lets path and type queries read crate identity and dependencies without
/// loading declaration tables.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateMetadata {
    target_kind: TargetKind,
    // Const arithmetic uses the compilation target, which may differ from the host.
    target_pointer_width: Option<u32>,
    name: String,
    root_module: ModuleId,
    extern_prelude: HashMap<Name, ModuleRef>,
    prelude: Option<ModuleRef>,
}

impl CrateMetadata {
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

    /// The builder supplies the root before publishing the completed crate.
    pub fn root_module(&self) -> ModuleId {
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

    /// Returns crate dependencies whose ordinary declarations participate in item lookup.
    ///
    /// Names are irrelevant to this traversal. Type queries walk these crate identities
    /// without decoding module scopes.
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
