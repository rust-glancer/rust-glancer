//! Collects the unresolved def-map skeleton from item trees.
//!
//! This phase walks one crate's module tree and records only what is directly visible from the
//! syntax:
//! - module hierarchy
//! - module-scope local definitions
//! - raw import directives
//! - immediate bindings such as child modules and `extern crate`
//!
//! Import resolution itself happens during build finalization, using the direct bindings produced
//! here. Collection also carries each logical module's filesystem context into queued macro calls.
//! That context is construction-only, but a later expansion needs it if it emits `mod child;`.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::Context as _;
use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::{CrateId, CrateRef, FileId, LocalDefId, ModuleId, ModuleRef, PackageSlot};
use rg_item_tree::{Package as ItemTreePackage, UserFacingAttrs};
use rg_parse::{ModuleFileContext, Package};
use rg_text::{Name, RustEdition};
use rg_workspace::TargetKind;

use super::{
    MacroSourceFileResolutions,
    macros::{
        MacroCallOrigin, MacroCallSite, MacroDefinitionRecord, MacroDirective, MacroDirectiveState,
        MacroUseImport, PendingGeneratedInclude, PendingGeneratedModule,
        PendingMacroExpansionLimitReport, TextualMacroScopes,
    },
};
use crate::{
    DefMapBuilder, MacroSourceFileRequest, ModuleData, ModuleDocumentation, ModuleOrigin,
    ModuleScope, Visibility,
};

mod declarations;
pub(crate) use self::declarations::DeclarationCollector;

/// Mutable state for one crate during declaration collection and fixed-point resolution.
///
/// The DefMap builder owns each module's structure, direct declarations, and resolved imports.
/// Crate-wide context and pending macro work stay here.
pub(super) struct CrateState {
    pub(super) crate_ref: CrateRef,
    pub(super) cargo_target: rg_parse::CargoTargetId,
    pub(super) crate_name: String,
    pub(super) root_module: ModuleId,
    pub(super) edition: RustEdition,
    /// Cargo-target-specific cfg values used to decide which collected items really exist.
    pub(super) cfg_options: CfgOptions,
    pub(super) target_kind: TargetKind,
    /// Package-local files available to context-sensitive module traversal.
    ///
    /// Construction refreshes this map after the source loader adds late
    /// source files. Frozen DefMaps retain only the selected `definition_file` ids.
    pub(super) known_module_files: Arc<KnownModuleFiles>,
    pub(super) def_map_builder: DefMapBuilder,
    pub(super) extern_prelude: ExternPreludeBuilder,
    pub(super) prelude: Option<ModuleRef>,
    pub(super) macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    pub(super) textual_macro_scopes: TextualMacroScopes,
    pub(super) macro_use_imports: Vec<MacroUseImport>,
    pub(super) macro_directives: Vec<MacroDirective>,
    /// Requests from the latest expansion batch, cleared after source loading.
    pub(super) macro_source_file_requests: Vec<MacroSourceFileRequest>,
    /// Generated module declarations waiting for the project boundary to capture their files.
    ///
    /// The surrounding macro has already been expanded and collected. Keeping this small
    /// pending operation lets collection finish without replaying that expansion or duplicating the
    /// other items it produced.
    pub(super) pending_generated_modules: Vec<PendingGeneratedModule>,
    /// Builtin includes waiting for the project boundary to capture their source files.
    pub(super) pending_generated_includes: Vec<PendingGeneratedInclude>,
    pub(super) macro_expansion_limit: Option<PendingMacroExpansionLimitReport>,
}

impl CrateState {
    pub(super) fn push_macro_call(&mut self, call: MacroCallSite, origin: MacroCallOrigin) {
        self.macro_directives.push(MacroDirective {
            call,
            origin,
            state: MacroDirectiveState::Pending,
        });
    }

    pub(super) fn cfg_evaluator(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(&self.cfg_options, self.target_kind.enables_test_cfg())
    }

    pub(super) fn resolve_module_file(
        &self,
        context: &ModuleFileContext,
        module_name: &str,
        path_override: Option<&str>,
    ) -> Option<(FileId, Arc<ModuleFileContext>)> {
        self.known_module_files
            .resolve(context, module_name, path_override)
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
pub(super) struct ExternPreludeBuilder {
    implicit_roots: HashMap<Name, ModuleRef>,
    explicit_aliases: HashMap<Name, ModuleRef>,
}

impl ExternPreludeBuilder {
    fn new(implicit_roots: &HashMap<Name, ModuleRef>) -> Self {
        Self {
            implicit_roots: implicit_roots.clone(),
            explicit_aliases: HashMap::new(),
        }
    }

    /// Resolve the source spelling of an `extern crate` declaration.
    pub(super) fn extern_crate_source(&self, name: &Name) -> Option<ModuleRef> {
        self.implicit_roots.get(name).copied()
    }

    /// Resolve a first path segment through explicit aliases, then Cargo-provided roots.
    pub(super) fn resolve(&self, name: &str) -> Option<ModuleRef> {
        self.explicit_aliases
            .get(name)
            .or_else(|| self.implicit_roots.get(name))
            .copied()
    }

    pub(super) fn insert_explicit_alias(&mut self, name: Name, module: ModuleRef) {
        self.explicit_aliases.insert(name, module);
    }

    /// Freeze the two construction sources into the query-facing extern prelude.
    pub(super) fn freeze(mut self) -> HashMap<Name, ModuleRef> {
        self.implicit_roots.extend(self.explicit_aliases);
        self.implicit_roots
    }
}

/// Canonical package paths used to connect syntax-only module declarations to parsed files.
pub(super) struct KnownModuleFiles {
    by_path: HashMap<PathBuf, FileId>,
}

impl KnownModuleFiles {
    pub(super) fn from_package(package: &Package, item_tree_package: &ItemTreePackage) -> Self {
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

pub(super) fn collect_package_crate_states(
    package_slot: usize,
    package: &Package,
    item_tree_package: &ItemTreePackage,
    implicit_roots: &[Vec<HashMap<Name, ModuleRef>>],
) -> anyhow::Result<Vec<CrateState>> {
    let mut package_states = Vec::with_capacity(package.targets().len());
    let known_module_files = Arc::new(KnownModuleFiles::from_package(package, item_tree_package));
    let resolutions = MacroSourceFileResolutions::default();
    for (crate_idx, target) in package.targets().iter().enumerate() {
        let crate_ref = CrateRef {
            package: PackageSlot(package_slot),
            crate_id: CrateId(crate_idx),
        };
        let roots = implicit_roots
            .get(package_slot)
            .and_then(|roots| roots.get(target.id.0))
            .expect("implicit roots should exist for every parsed target");
        let target_root = item_tree_package
            .target_root(target.id)
            .context("target root should be lowered")?;
        let root_file = item_tree_package
            .file(target_root.root_file)
            .context("target root file should be lowered")?;
        let mut builder = DefMapBuilder::new(crate_ref);
        // A target root has neither a declaration name nor a parent. All descendants use the
        // shared collector, just like files and declarations discovered by macro expansion.
        let root_module = builder.alloc_module(ModuleData {
            name: None,
            name_span: None,
            docs: ModuleDocumentation::new(None, root_file.docs.clone()),
            user_facing_attrs: UserFacingAttrs::default(),
            visibility: Visibility::Public,
            parent: None,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin: ModuleOrigin::Root {
                file_id: target.root_file,
            },
        });
        let mut state = CrateState {
            crate_ref,
            cargo_target: target.id,
            crate_name: target.name.clone(),
            root_module,
            edition: package.edition(),
            cfg_options: package.cfg_options().clone(),
            target_kind: target.kind.clone(),
            known_module_files: Arc::clone(&known_module_files),
            def_map_builder: builder,
            extern_prelude: ExternPreludeBuilder::new(roots),
            prelude: None,
            macro_definitions: HashMap::new(),
            textual_macro_scopes: TextualMacroScopes::default(),
            macro_use_imports: Vec::new(),
            macro_directives: Vec::new(),
            macro_source_file_requests: Vec::new(),
            pending_generated_modules: Vec::new(),
            pending_generated_includes: Vec::new(),
            macro_expansion_limit: None,
        };
        let context = Arc::new(ModuleFileContext::for_target_root(
            package
                .file_path(target_root.root_file)
                .expect("target root should have a parsed path"),
        ));
        DeclarationCollector::new(
            &mut state,
            item_tree_package,
            &resolutions,
            MacroCallOrigin::Source,
            None,
        )
        .collect_module_file(root_module, target_root.root_file, context)
        .with_context(|| format!("collect crate scope for {}", target.name))?;
        package_states.push(state);
    }
    Ok(package_states)
}
