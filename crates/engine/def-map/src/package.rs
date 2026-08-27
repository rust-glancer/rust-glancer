use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{CrateId, CrateRef, ModuleId, ModuleRef};
use rg_parse::{CargoTargetId, FileId};
use rg_std::{MemorySize, Shrink, UniqueVec};
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

/// Routing information needed before one crate DefMap is decoded.
///
/// A source file can belong to more than one Cargo target, such as a library also compiled through
/// an example target. This entry records every file mentioned by the crate's module origins so a
/// query can choose all matching [`CrateRef`]s without reading their scopes. It also carries the
/// proc-macro boundary and dependency identities needed by cross-crate item lookup.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateDefMapManifest {
    cargo_target: CargoTargetId,
    files: UniqueVec<FileId>,
    is_proc_macro: bool,
    item_lookup_dependencies: UniqueVec<CrateRef>,
}

impl CrateDefMapManifest {
    pub fn cargo_target(&self) -> CargoTargetId {
        self.cargo_target
    }

    pub fn files(&self) -> &[FileId] {
        self.files.as_slice()
    }

    pub fn is_proc_macro(&self) -> bool {
        self.is_proc_macro
    }

    pub fn item_lookup_dependencies(&self) -> &UniqueVec<CrateRef> {
        &self.item_lookup_dependencies
    }
}

/// Package directory retained while the full DefMap payload is offloaded.
///
/// Crate entries stay in [`CrateId`] order. Together they can answer package metadata, file routing,
/// proc-macro classification, and dependency traversal without loading module scopes. The directory
/// intentionally contains neither definitions from those scopes nor diagnostics from the build that
/// produced them.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageDefMapsManifest {
    name: String,
    edition: RustEdition,
    crates: Arena<CrateId, CrateDefMapManifest>,
}

impl PackageDefMapsManifest {
    pub fn package_name(&self) -> &str {
        &self.name
    }

    pub fn edition(&self) -> RustEdition {
        self.edition
    }

    pub fn crates(&self) -> &[CrateDefMapManifest] {
        self.crates.as_slice()
    }

    pub fn crate_manifest(&self, crate_id: CrateId) -> Option<&CrateDefMapManifest> {
        self.crates.get(crate_id)
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

    /// Extracts the routing directory stored in front of the crate-granular DefMap payloads.
    ///
    /// Only facts needed before an exact crate is known are copied here. Module scopes remain in
    /// their crate payload so a package with many targets does not make every query pay for them.
    pub fn manifest(&self) -> PackageDefMapsManifest {
        let crates = self
            .crates
            .iter()
            .map(|crate_data| {
                // Module origins may mention the same declaration file many times through inline
                // modules. Canonical ordering keeps persisted routing deterministic, while the
                // ordered set keeps only one copy of each file.
                let mut files = crate_data
                    .def_map()
                    .modules()
                    .iter()
                    .flat_map(|module| module.origin.files())
                    .collect::<Vec<_>>();
                files.sort_by_key(|file| file.0);
                let files = files.into_iter().collect::<UniqueVec<_>>();
                CrateDefMapManifest {
                    cargo_target: crate_data.cargo_target(),
                    files,
                    is_proc_macro: crate_data.is_proc_macro(),
                    item_lookup_dependencies: crate_data.item_lookup_dependencies(),
                }
            })
            .collect();

        PackageDefMapsManifest {
            name: self.name.clone(),
            edition: self.edition,
            crates: Arena::from_vec(crates),
        }
    }

    /// Rebuilds the broad resident package from every crate-granular storage unit.
    ///
    /// The manifest owns the package-level metadata. Each crate payload must occupy the same dense
    /// slot and retain the Cargo target declared by its manifest entry; otherwise the artifact is
    /// stale or malformed and cannot safely be exposed as a package.
    pub fn from_storage_parts(
        manifest: &PackageDefMapsManifest,
        crates: Vec<CrateData>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            crates.len() == manifest.crates.len(),
            "DefMap storage has {} crates, manifest declares {}",
            crates.len(),
            manifest.crates.len(),
        );
        for (crate_idx, (crate_data, crate_manifest)) in
            crates.iter().zip(manifest.crates.iter()).enumerate()
        {
            anyhow::ensure!(
                crate_data.cargo_target() == crate_manifest.cargo_target,
                "DefMap crate {crate_idx} belongs to Cargo target {:?}, manifest declares {:?}",
                crate_data.cargo_target(),
                crate_manifest.cargo_target,
            );
        }
        Ok(Self {
            name: manifest.name.clone(),
            edition: manifest.edition,
            crates: Arena::from_vec(crates),
            // Macro-limit reports describe the build operation rather than semantic declarations.
            // They leave memory when a package is offloaded instead of being restored from cache.
            macro_expansion_limits: Vec::new(),
        })
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
/// retain while the resident package is available for diagnostics.
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
