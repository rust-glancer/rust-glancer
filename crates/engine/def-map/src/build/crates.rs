//! Mutable crate ownership during DefMap construction.
//!
//! Collection, import resolution, and macro expansion work on the same builders. File loading can
//! grow their source context without replacing the builders or the module scopes they already own.
//! Freezing moves only settled crate data into package payloads; macro queues and file maps stay here.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::{CrateRef, DefId, FileId, LocalDefId, ModuleId, ModuleRef, PackageSlot};
use rg_item_tree::{ItemTreeDb, Package as ItemTreePackage};
use rg_parse::{ModuleFileContext, Package};
use rg_std::UniqueVec;
use rg_text::{Name, RustEdition};
use rg_workspace::TargetKind;

use super::macros::{
    MacroCallOrigin, MacroCallSite, MacroDefinitionRecord, MacroDirective, MacroDirectiveState,
    MacroUseImport, PendingGeneratedInclude, PendingGeneratedModule,
    PendingMacroExpansionLimitReport, TextualMacroScopes,
};
use crate::{
    CrateData, DefMapBuilder, GeneratedItemStores, MacroExpansionLimitReport,
    MacroSourceFileRequest, ModuleScopeBuilder, Namespace, PackageDefMaps,
};

/// Mutable crate builders indexed by package slot and Cargo target.
///
/// Selected packages own fresh builders. Absent package slots are read through the frozen baseline
/// by `BuildResolutionEnv`. This store stays alive across import waves, macro batches, and source
/// loading; only `freeze_package` moves settled data into a retained package.
pub(crate) struct CrateBuildStore {
    packages: Vec<Option<Vec<CrateBuilder>>>,
}

impl CrateBuildStore {
    pub(crate) fn new(package_count: usize) -> Self {
        Self {
            packages: (0..package_count).map(|_| None).collect(),
        }
    }

    pub(crate) fn replace_package(
        &mut self,
        package: PackageSlot,
        states: Vec<CrateBuilder>,
    ) -> Option<()> {
        *self.packages.get_mut(package.0)? = Some(states);
        Some(())
    }

    pub(crate) fn package(&self, package: PackageSlot) -> Option<&[CrateBuilder]> {
        self.packages.get(package.0)?.as_deref()
    }

    pub(crate) fn crate_builder(&self, crate_ref: CrateRef) -> Option<&CrateBuilder> {
        self.package(crate_ref.package)?.get(crate_ref.crate_id.0)
    }

    pub(crate) fn crate_builder_mut(&mut self, crate_ref: CrateRef) -> Option<&mut CrateBuilder> {
        self.packages
            .get_mut(crate_ref.package.0)?
            .as_deref_mut()?
            .get_mut(crate_ref.crate_id.0)
    }

    pub(crate) fn iter_dirty(&self) -> impl Iterator<Item = &[CrateBuilder]> {
        self.packages.iter().filter_map(Option::as_deref)
    }

    pub(crate) fn iter_dirty_mut(&mut self) -> impl Iterator<Item = &mut [CrateBuilder]> {
        self.packages.iter_mut().filter_map(Option::as_deref_mut)
    }

    pub(crate) fn iter_dirty_mut_enumerated(
        &mut self,
    ) -> impl Iterator<Item = (usize, &mut [CrateBuilder])> {
        self.packages
            .iter_mut()
            .enumerate()
            .filter_map(|(package_slot, states)| {
                states.as_deref_mut().map(|states| (package_slot, states))
            })
    }

    /// Clears the source batch after the loader has supplied its results.
    pub(crate) fn clear_macro_source_file_requests(&mut self) {
        for package_states in self.iter_dirty_mut() {
            for state in package_states {
                state.macro_source_file_requests.clear();
            }
        }
    }

    /// Refreshes known package files after late Parse/ItemTree growth.
    pub(crate) fn refresh_known_module_files(
        &mut self,
        packages: &[Package],
        item_tree: &ItemTreeDb,
    ) {
        for (package_slot, package_states) in self.iter_dirty_mut_enumerated() {
            let item_tree_package = item_tree
                .package(package_slot)
                .expect("dirty package should have an item tree during construction");
            let known_module_files = Arc::new(KnownModuleFiles::from_package(
                &packages[package_slot],
                item_tree_package,
            ));
            for state in package_states {
                state.known_module_files = Arc::clone(&known_module_files);
            }
        }
    }

    /// Coalesces construction requests without retaining them in frozen package payloads.
    pub(crate) fn macro_source_file_requests(&self) -> Vec<MacroSourceFileRequest> {
        let mut requests = UniqueVec::new();

        for package_states in self.iter_dirty() {
            for state in package_states {
                for request in &state.macro_source_file_requests {
                    requests.push(request.clone());
                }
            }
        }

        requests.into_vec()
    }

    pub(crate) fn module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.crate_builder(module.origin.as_crate_ref()?)?
            .def_map_builder
            .module_scope(module.module)
    }

    /// Keep proc-macro implementation items private before another crate can import them.
    pub(crate) fn censor_proc_macro_exports(&mut self) {
        for package_states in self.iter_dirty_mut() {
            for state in package_states {
                if state.target_kind != TargetKind::ProcMacro {
                    continue;
                }
                // Censoring reads declaration metadata while changing the resolved scope. Move the
                // scope out briefly so those borrows stay separate without cloning its bindings.
                let root = state.root_module;
                let mut scope = std::mem::take(
                    state
                        .def_map_builder
                        .module_scope_mut(root)
                        .expect("proc-macro root scope should exist"),
                );
                state.censor_proc_macro_scope(root, &mut scope);
                *state
                    .def_map_builder
                    .module_scope_mut(root)
                    .expect("proc-macro root scope should remain allocated") = scope;
            }
        }
    }

    /// Move a settled package into retained data and separate its transient generated declarations.
    pub(crate) fn freeze_package(
        &mut self,
        slot: PackageSlot,
        package: &Package,
        generated_items: &mut GeneratedItemStores,
    ) -> PackageDefMaps {
        let package_states = self
            .packages
            .get_mut(slot.0)
            .and_then(Option::take)
            .expect("selected package should have completed crate builders");
        let package_name = package.package_name().to_string();
        let mut crates = Vec::with_capacity(package_states.len());
        let mut macro_expansion_limits = Vec::new();

        // Split each builder by moving its frozen DefMap into the package and its generated syntax into
        // the transient phase output. This avoids both cloning the expansion arena and materializing an
        // intermediate collection that the caller would immediately consume.
        for state in package_states {
            if let Some(report) = &state.macro_expansion_limit {
                macro_expansion_limits.push(MacroExpansionLimitReport {
                    package_name: package_name.clone(),
                    crate_name: state.crate_name.clone(),
                    groups: report.groups.clone(),
                    omitted_call_count: report.omitted_call_count,
                });
            }

            let crate_ref = state.crate_ref;
            let (def_map, crate_generated_items) = state.def_map_builder.into_parts();
            generated_items.insert(crate_ref, crate_generated_items);

            // Const arithmetic also needs the compilation target's usize width after parsed package
            // data is offloaded. Missing target information must not fall back to the host width.
            let target_pointer_width = state
                .cfg_options
                .key_values()
                .iter()
                .find(|value| value.key() == "target_pointer_width")
                .and_then(|value| value.value().parse().ok());

            // Persist both Cargo-provided roots and explicit crate-root aliases. Queries read this as a
            // prelude rather than pretending any of these names are child modules of the crate root.
            crates.push(CrateData::new(
                state.cargo_target,
                state.target_kind,
                target_pointer_width,
                state.crate_name,
                Some(state.root_module),
                state.extern_prelude.freeze(),
                state.prelude,
                def_map,
            ));
        }

        PackageDefMaps::new(
            package_name,
            package.edition(),
            crates,
            macro_expansion_limits,
        )
    }
}

/// Collects one crate's declarations and build-only macro work before freezing.
///
/// The DefMap builder owns each module's structure, direct declarations, and resolved imports.
/// Crate-wide context and pending macro work stay here.
pub(crate) struct CrateBuilder {
    pub(crate) crate_ref: CrateRef,
    pub(crate) cargo_target: rg_parse::CargoTargetId,
    pub(crate) crate_name: String,
    pub(crate) root_module: ModuleId,
    pub(crate) edition: RustEdition,
    /// Cargo-target-specific cfg values used to decide which collected items really exist.
    pub(crate) cfg_options: CfgOptions,
    pub(crate) target_kind: TargetKind,
    /// Package-local files available to context-sensitive module traversal.
    ///
    /// Construction refreshes this map after the source loader adds late
    /// source files. Frozen DefMaps retain only the selected `definition_file` ids.
    pub(crate) known_module_files: Arc<KnownModuleFiles>,
    pub(crate) def_map_builder: DefMapBuilder,
    pub(crate) extern_prelude: ExternPreludeBuilder,
    pub(crate) prelude: Option<ModuleRef>,
    pub(crate) macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    pub(crate) textual_macro_scopes: TextualMacroScopes,
    pub(crate) macro_use_imports: Vec<MacroUseImport>,
    pub(crate) macro_directives: Vec<MacroDirective>,
    /// Requests from the latest expansion batch, cleared after source loading.
    pub(crate) macro_source_file_requests: Vec<MacroSourceFileRequest>,
    /// Generated module declarations waiting for the project boundary to capture their files.
    ///
    /// The surrounding macro has already been expanded and collected. Keeping this small
    /// pending operation lets collection finish without replaying that expansion or duplicating the
    /// other items it produced.
    pub(crate) pending_generated_modules: Vec<PendingGeneratedModule>,
    /// Builtin includes waiting for the project boundary to capture their source files.
    pub(crate) pending_generated_includes: Vec<PendingGeneratedInclude>,
    pub(crate) macro_expansion_limit: Option<PendingMacroExpansionLimitReport>,
}

impl CrateBuilder {
    pub(crate) fn push_macro_call(&mut self, call: MacroCallSite, origin: MacroCallOrigin) {
        self.macro_directives.push(MacroDirective {
            call,
            origin,
            state: MacroDirectiveState::Pending,
        });
    }

    pub(crate) fn cfg_evaluator(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(&self.cfg_options, self.target_kind.enables_test_cfg())
    }

    pub(crate) fn resolve_module_file(
        &self,
        context: &ModuleFileContext,
        module_name: &str,
        path_override: Option<&str>,
    ) -> Option<(FileId, Arc<ModuleFileContext>)> {
        self.known_module_files
            .resolve(context, module_name, path_override)
    }

    /// Make one rebuilt scope obey the external export rules of a proc-macro crate.
    ///
    /// A function marked `#[proc_macro]` is a normal value inside its own source crate, but another
    /// crate may see only its macro identity. Worklist candidates call this before comparison so
    /// the hidden function value cannot look like a scope change and wake more import groups.
    pub(crate) fn censor_proc_macro_scope(&self, module: ModuleId, scope: &mut ModuleScopeBuilder) {
        if self.target_kind != TargetKind::ProcMacro || module != self.root_module {
            return;
        }

        let root = ModuleRef::krate(self.crate_ref, self.root_module);
        let def_map = &self.def_map_builder;
        scope.censor_public_bindings(root, |namespace, binding| {
            if namespace != Namespace::Macros || !binding.has_direct_declaration() {
                return false;
            }

            let DefId::Local(local_def) = binding.def else {
                return false;
            };
            local_def.origin == root.origin
                && def_map
                    .macro_definition(local_def.local_def)
                    .and_then(|data| data.proc_macro_implementation())
                    .is_some()
        });
    }
}

/// Mutable crate-wide extern prelude assembled while DefMap is built.
///
/// Cargo dependency names provide the initial roots. A root declaration such as
/// `extern crate alloc as alloc_crate;` then adds `alloc_crate` for every real module in the
/// crate. Rust gives only crate-root `extern crate` declarations this behavior; ordinary root
/// items and declarations inside nested modules do not become unqualified child-module names.
///
/// Explicit aliases stay separate while collection is mutable. The source name in a later
/// `extern crate` declaration must still name a Cargo dependency, not an alias introduced by an
/// earlier declaration.
pub(crate) struct ExternPreludeBuilder {
    implicit_roots: HashMap<Name, ModuleRef>,
    explicit_aliases: HashMap<Name, ModuleRef>,
}

impl ExternPreludeBuilder {
    pub(crate) fn new(implicit_roots: &HashMap<Name, ModuleRef>) -> Self {
        Self {
            implicit_roots: implicit_roots.clone(),
            explicit_aliases: HashMap::new(),
        }
    }

    /// Resolve the source spelling of an `extern crate` declaration.
    pub(crate) fn extern_crate_source(&self, name: &Name) -> Option<ModuleRef> {
        self.implicit_roots.get(name).copied()
    }

    /// Resolve a first path segment through explicit aliases, then Cargo-provided roots.
    pub(crate) fn resolve(&self, name: &str) -> Option<ModuleRef> {
        self.explicit_aliases
            .get(name)
            .or_else(|| self.implicit_roots.get(name))
            .copied()
    }

    pub(crate) fn insert_explicit_alias(&mut self, name: Name, module: ModuleRef) {
        self.explicit_aliases.insert(name, module);
    }

    /// Freeze the two construction sources into the query-facing extern prelude.
    pub(crate) fn freeze(mut self) -> HashMap<Name, ModuleRef> {
        self.implicit_roots.extend(self.explicit_aliases);
        self.implicit_roots
    }
}

/// Canonical package paths used to connect syntax-only module declarations to parsed files.
pub(crate) struct KnownModuleFiles {
    by_path: HashMap<PathBuf, FileId>,
}

impl KnownModuleFiles {
    pub(crate) fn from_package(package: &Package, item_tree_package: &ItemTreePackage) -> Self {
        Self {
            // Parse retains stable ids for historical files across saved rebuilds. Only files with
            // a current ItemTree belong to this generation's reachable module graph.
            by_path: item_tree_package
                .files()
                .map(|file| {
                    let path = package
                        .file_path(file.file)
                        .expect("lowered file should have a parsed path")
                        .to_path_buf();
                    (path, file.file)
                })
                .collect(),
        }
    }

    fn resolve(
        &self,
        context: &ModuleFileContext,
        module_name: &str,
        path_override: Option<&str>,
    ) -> Option<(FileId, Arc<ModuleFileContext>)> {
        context
            .resolve_known_module_name(module_name, path_override, |path| {
                self.by_path.get(path).copied().or_else(|| {
                    let canonical_path = path.canonicalize().ok()?;
                    self.by_path.get(&canonical_path).copied()
                })
            })
            .map(|(file_id, context)| (file_id, Arc::new(context)))
    }
}
