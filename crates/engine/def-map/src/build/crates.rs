//! Mutable crate ownership during DefMap construction.
//!
//! Collection, import resolution, and macro expansion work on the active builder. File loading can
//! grow its source context without replacing its module scopes. Finishing consumes the builder,
//! keeping frozen crate data and generated declarations while dropping the build-only queues.

use std::{collections::HashMap, mem, path::PathBuf, sync::Arc};

use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::{CrateRef, DefId, FileId, LocalDefId, ModuleId, ModuleRef};
use rg_item_tree::Package as ItemTreePackage;
use rg_parse::{CargoTargetId, ModuleFileContext, Package};
use rg_text::{Name, RustEdition};
use rg_workspace::TargetKind;

use super::{
    macros::{
        MacroCallOrigin, MacroCallSite, MacroDefinitionRecord, MacroDirective, MacroDirectiveState,
        MacroUseImport, PendingGeneratedInclude, PendingGeneratedModule,
        PendingMacroExpansionLimitReport, TextualMacroScopes,
    },
    output::CrateBuildOutput,
};
use crate::{
    CrateData, DefMapBuilder, ItemScopeBuilder, MacroExpansionLimitReport, MacroSourceFileRequest,
    NameBinding, Namespace,
};

/// Collects one crate's declarations and build-only macro work before freezing.
///
/// The DefMap builder owns each module's structure, direct declarations, and resolved imports.
/// Crate-wide context and pending macro work stay here.
pub(crate) struct CrateBuilder {
    pub(crate) crate_ref: CrateRef,
    pub(crate) cargo_target: CargoTargetId,
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
    /// Legacy macro imports sorted by declaration order, including generated declarations.
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
    /// Move settled data into the result and discard build-only queues and textual macro state.
    /// Generated declarations remain separate because Semantic IR still needs their item syntax.
    pub(crate) fn finish(self, package_name: &str) -> CrateBuildOutput {
        let macro_expansion_limit =
            self.macro_expansion_limit
                .map(|report| MacroExpansionLimitReport {
                    package_name: package_name.to_owned(),
                    crate_name: self.crate_name.clone(),
                    groups: report.groups,
                    omitted_call_count: report.omitted_call_count,
                });
        // Const arithmetic must retain the compilation target's width after Parse is offloaded.
        let target_pointer_width = self
            .cfg_options
            .key_values()
            .iter()
            .find(|value| value.key() == "target_pointer_width")
            .and_then(|value| value.value().parse().ok());
        let (def_map, generated_items) = self.def_map_builder.into_parts();
        let data = CrateData::new(
            self.cargo_target,
            self.target_kind,
            target_pointer_width,
            self.crate_name,
            Some(self.root_module),
            self.extern_prelude.freeze(),
            self.prelude,
            def_map,
        );
        CrateBuildOutput {
            crate_ref: self.crate_ref,
            data,
            generated_items,
            macro_expansion_limit,
        }
    }

    /// Keep the proc-macro implementation private before exposing the completed crate.
    pub(crate) fn censor_proc_macro_exports(&mut self) {
        if self.target_kind != TargetKind::ProcMacro {
            return;
        }
        let root = self.root_module;
        let mut scope = mem::take(
            self.def_map_builder
                .module_scope_mut(root)
                .expect("proc-macro root scope exists"),
        );
        self.censor_proc_macro_scope(root, &mut scope);
        *self
            .def_map_builder
            .module_scope_mut(root)
            .expect("proc-macro root scope exists") = scope;
    }

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

    /// Make a collected scope obey the external export rules of a proc-macro crate.
    ///
    /// A function marked `#[proc_macro]` is a normal value inside its own source crate, but another
    /// crate may see only its macro identity. Apply the same export boundary to initial
    /// declarations and to imported bindings before other crates read them.
    pub(crate) fn censor_proc_macro_scope(&self, module: ModuleId, scope: &mut ItemScopeBuilder) {
        if self.target_kind != TargetKind::ProcMacro || module != self.root_module {
            return;
        }

        let root = ModuleRef::krate(self.crate_ref, self.root_module);
        scope.censor_public_bindings(root, |namespace, binding| {
            self.allow_public_binding(module, namespace, binding)
        });
    }

    /// Check what may stay public at a proc-macro crate root.
    /// Initial scope censoring and later import insertions share this rule, so an imported name
    /// cannot bypass the export boundary after other crates have started reading the scope.
    pub(crate) fn allow_public_binding(
        &self,
        module: ModuleId,
        namespace: Namespace,
        binding: &NameBinding,
    ) -> bool {
        if self.target_kind != TargetKind::ProcMacro || module != self.root_module {
            return true;
        }
        let root = ModuleRef::krate(self.crate_ref, self.root_module);
        if namespace != Namespace::Macros || !binding.has_direct_declaration() {
            return false;
        }
        let DefId::Local(local) = binding.def else {
            return false;
        };
        local.origin == root.origin
            && self
                .def_map_builder
                .macro_definition(local.local_def)
                .and_then(|data| data.proc_macro_implementation())
                .is_some()
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
    pub(crate) fn new(implicit_roots: HashMap<Name, ModuleRef>) -> Self {
        Self {
            implicit_roots,
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
