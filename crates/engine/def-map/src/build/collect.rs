//! Collects the unresolved def-map skeleton from item trees.
//!
//! This phase walks one crate's module tree and records only what is directly visible from the
//! syntax:
//! - module hierarchy
//! - module-scope local definitions
//! - raw import directives
//! - immediate bindings such as child modules and `extern crate`
//!
//! Import resolution itself happens during build finalization, using the `base_scopes` produced
//! here. Collection also carries each logical module's filesystem context into queued macro calls.
//! That context is construction-only, but a later expansion needs it if it emits `mod child;`.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context as _;

use crate::{
    DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, LocalDefData, LocalDefKind,
    LocalImplData, MacroDefinitionData, ModuleData, ModuleFileSelection, ModuleOrigin, ModuleScope,
    ModuleScopeBuilder, Namespace, NamespaceSet, ScopeBinding, ScopeBindingProvenance, Visibility,
};
use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::{
    CrateId, CrateRef, DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef,
};
use rg_item_tree::{
    Documentation, EnumItem, ExternBlockItem, ExternCrateItem, FunctionItem, ItemKind, ItemNode,
    ItemTreeId, ItemTreeRef, MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem, ModuleItem,
    ModuleSource, Package as ItemTreePackage, UseImport, UseItem, UserFacingAttrs, VisibilityLevel,
};
use rg_parse::{CargoTarget, ModuleFileContext, Package};
use rg_text::{Name, RustEdition};
use rg_workspace::TargetKind;

use crate::MacroSourceFileRequest;
use crate::PackageSlot;

use super::macros::{
    ItemOrder, MacroCallOrigin, MacroCallPlacement, MacroCallSite, MacroDefinitionRecord,
    MacroDirective, MacroDirectiveState, MacroUseImport, PendingGeneratedInclude,
    PendingGeneratedModule, PendingMacroExpansionLimitReport, TextualMacroScopes,
};

/// Collected state for one crate before fixed-point import resolution.
///
/// `def_map` contains the frozen structural data, while `base_scopes` keeps the directly known
/// bindings that later passes start from.
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
    /// Resumable finalization refreshes this construction-only map after the project adds late
    /// source files. Frozen DefMaps retain only the selected `definition_file` ids.
    pub(super) known_module_files: Arc<KnownModuleFiles>,
    pub(super) def_map_builder: DefMapBuilder,
    pub(super) base_scopes: Vec<ModuleScopeBuilder>,
    pub(super) extern_prelude: ExternPreludeBuilder,
    pub(super) prelude: Option<ModuleRef>,
    pub(super) macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    pub(super) textual_macro_scopes: TextualMacroScopes,
    pub(super) macro_use_imports: Vec<MacroUseImport>,
    pub(super) macro_directives: Vec<MacroDirective>,
    /// Requests emitted by the latest finalization step and cleared before the next resume.
    pub(super) macro_source_file_requests: Vec<MacroSourceFileRequest>,
    /// Generated module declarations waiting for the project boundary to capture their files.
    ///
    /// The surrounding macro has already been expanded and collected. Keeping this small
    /// continuation lets construction resume without replaying that expansion or duplicating the
    /// other items it produced.
    pub(super) pending_generated_modules: Vec<PendingGeneratedModule>,
    /// Builtin includes waiting for the project boundary to capture their source files.
    pub(super) pending_generated_includes: Vec<PendingGeneratedInclude>,
    pub(super) macro_expansion_limit: Option<PendingMacroExpansionLimitReport>,
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
    ) -> Option<(rg_parse::FileId, Arc<ModuleFileContext>)> {
        self.known_module_files
            .resolve(context, module_name, path_override)
    }
}

/// Canonical package paths used to connect syntax-only module declarations to parsed files.
pub(super) struct KnownModuleFiles {
    by_path: HashMap<PathBuf, rg_parse::FileId>,
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
    ) -> Option<(rg_parse::FileId, Arc<ModuleFileContext>)> {
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

    for (crate_idx, target) in package.targets().iter().enumerate() {
        let crate_ref = CrateRef {
            package: PackageSlot(package_slot),
            crate_id: CrateId(crate_idx),
        };
        let crate_roots = implicit_roots
            .get(package_slot)
            .and_then(|package_roots| package_roots.get(target.id.0))
            .expect("implicit roots should exist for every parsed target");
        let target_root = item_tree_package.target_root(target.id).with_context(|| {
            format!(
                "while attempting to fetch item tree target root for {}",
                target.name
            )
        })?;

        let collector = CrateScopeCollector::new(
            crate_ref,
            target.id,
            package.edition(),
            package.cfg_options(),
            target.kind.clone(),
            crate_roots,
            Arc::clone(&known_module_files),
        );
        let state = collector
            .collect(
                item_tree_package,
                target,
                target_root.root_file,
                Arc::new(ModuleFileContext::for_target_root(
                    package
                        .file_path(target_root.root_file)
                        .expect("target root should have a parsed path"),
                )),
            )
            .with_context(|| {
                format!(
                    "while attempting to collect crate scope for {}",
                    target.name
                )
            })?;
        package_states.push(state);
    }

    Ok(package_states)
}

/// Mutable collector for one crate's module tree.
///
/// The collector builds two parallel structures:
/// - `def_map.modules`, which is the final structural payload
/// - `base_scopes`, which starts with only directly known bindings and is enriched later
struct CrateScopeCollector<'db> {
    crate_ref: CrateRef,
    cargo_target: rg_parse::CargoTargetId,
    edition: RustEdition,
    cfg_options: &'db CfgOptions,
    target_kind: TargetKind,
    known_module_files: Arc<KnownModuleFiles>,
    extern_prelude: ExternPreludeBuilder,
    root_module: Option<ModuleId>,
    def_map_builder: DefMapBuilder,
    base_scopes: Vec<ModuleScopeBuilder>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    textual_macro_scopes: TextualMacroScopes,
    macro_use_imports: Vec<MacroUseImport>,
    macro_directives: Vec<MacroDirective>,
    active_files: HashSet<rg_parse::FileId>,
}

impl<'db> CrateScopeCollector<'db> {
    fn new(
        crate_ref: CrateRef,
        cargo_target: rg_parse::CargoTargetId,
        edition: RustEdition,
        cfg_options: &'db CfgOptions,
        target_kind: TargetKind,
        implicit_roots: &'db HashMap<Name, ModuleRef>,
        known_module_files: Arc<KnownModuleFiles>,
    ) -> Self {
        Self {
            crate_ref,
            cargo_target,
            edition,
            cfg_options,
            target_kind,
            known_module_files,
            extern_prelude: ExternPreludeBuilder::new(implicit_roots),
            root_module: None,
            def_map_builder: DefMapBuilder::new(crate_ref),
            base_scopes: Vec::new(),
            macro_definitions: HashMap::new(),
            textual_macro_scopes: TextualMacroScopes::default(),
            macro_use_imports: Vec::new(),
            macro_directives: Vec::new(),
            active_files: HashSet::default(),
        }
    }

    /// Walks the target starting from its root file and returns the unresolved crate state.
    fn collect(
        mut self,
        item_tree: &ItemTreePackage,
        target: &CargoTarget,
        root_file: rg_parse::FileId,
        root_context: Arc<ModuleFileContext>,
    ) -> anyhow::Result<CrateState> {
        let root_file_tree = item_tree.file(root_file).with_context(|| {
            format!(
                "while attempting to fetch root item tree for {:?}",
                root_file
            )
        })?;
        // Root modules are identified by the target; they do not have a textual name or parent.
        let root_module = self.alloc_module(
            None,
            None,
            None,
            root_file_tree.docs.clone(),
            UserFacingAttrs::default(),
            Visibility::Public,
            ModuleOrigin::Root {
                file_id: target.root_file,
            },
        );
        self.root_module = Some(root_module);

        let inserted = self.active_files.insert(root_file);
        debug_assert!(
            inserted,
            "a fresh collector should not have an active root file"
        );
        let collected = self.collect_items_in_context(
            item_tree,
            root_module,
            root_file,
            &root_file_tree.top_level,
            root_context,
        );
        self.active_files.remove(&root_file);
        collected.context("while attempting to collect root file items")?;

        Ok(CrateState {
            crate_ref: self.crate_ref,
            cargo_target: self.cargo_target,
            crate_name: target.name.clone(),
            root_module,
            edition: self.edition,
            cfg_options: self.cfg_options.clone(),
            target_kind: self.target_kind.clone(),
            known_module_files: self.known_module_files,
            def_map_builder: self.def_map_builder,
            base_scopes: self.base_scopes,
            extern_prelude: self.extern_prelude,
            prelude: None,
            macro_definitions: self.macro_definitions,
            textual_macro_scopes: self.textual_macro_scopes,
            macro_use_imports: self.macro_use_imports,
            macro_directives: self.macro_directives,
            macro_source_file_requests: Vec::new(),
            pending_generated_modules: Vec::new(),
            pending_generated_includes: Vec::new(),
            macro_expansion_limit: None,
        })
    }

    /// Allocates one module in both the def-map payload and the base-scope table.
    #[allow(clippy::too_many_arguments)]
    fn alloc_module(
        &mut self,
        parent: Option<ModuleId>,
        name: Option<Name>,
        name_span: Option<rg_parse::Span>,
        docs: Option<rg_item_tree::Documentation>,
        user_facing_attrs: UserFacingAttrs,
        visibility: Visibility,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let module_id = self.def_map_builder.alloc_module(ModuleData {
            name,
            name_span,
            docs,
            user_facing_attrs,
            visibility,
            parent,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin,
        });
        self.base_scopes.push(ModuleScopeBuilder::default());
        module_id
    }

    /// Walks items while preserving the filesystem base of this logical module.
    ///
    /// Inline children descend from this base, macro calls retain it until expansion, and an
    /// out-of-line declaration supplies the next context selected along that module edge.
    fn collect_items_in_context(
        &mut self,
        item_tree: &ItemTreePackage,
        module_id: ModuleId,
        file_id: rg_parse::FileId,
        items: &[ItemTreeId],
        module_file_context: Arc<ModuleFileContext>,
    ) -> anyhow::Result<()> {
        for (item_index, item_id) in items.iter().enumerate() {
            let source = ItemTreeRef {
                file_id,
                item: *item_id,
            };
            let order = ItemOrder::real(item_index);
            let item = item_tree
                .item(source)
                .expect("item tree id should exist while collecting def map");
            if !self.is_item_enabled(item) {
                // Disabled items should not leave partial scope data behind. This removes the item
                // itself together with nested modules, imports, and macro directives.
                continue;
            }
            match &item.kind {
                ItemKind::ExternBlock(extern_block) => {
                    self.collect_extern_block(item_tree, module_id, source, extern_block);
                }
                ItemKind::ExternCrate(extern_crate) => {
                    self.collect_extern_crate(module_id, item, extern_crate);
                }
                ItemKind::Module(module_item) => {
                    self.collect_module(
                        item_tree,
                        module_id,
                        item,
                        module_item,
                        order,
                        Arc::clone(&module_file_context),
                    )
                    .with_context(|| {
                        format!(
                            "while attempting to collect module {}",
                            item.name.as_deref().unwrap_or("<unnamed>")
                        )
                    })?;
                }
                ItemKind::Use(use_item) => {
                    self.collect_use(module_id, item, source, use_item);
                }
                ItemKind::Impl(impl_item) => {
                    self.collect_local_impl(module_id, item, source);
                    self.collect_associated_macro_calls(
                        item_tree,
                        module_id,
                        source,
                        &impl_item.items,
                        &order,
                        &module_file_context,
                    );
                }
                ItemKind::MacroCall(macro_call) => {
                    self.collect_macro_call(
                        module_id,
                        item,
                        source,
                        macro_call,
                        order,
                        MacroCallPlacement::ModuleItems,
                        Arc::clone(&module_file_context),
                    );
                }
                ItemKind::MacroDefinition(macro_definition) => {
                    self.collect_macro_definition(module_id, item, source, macro_definition, order);
                }
                ItemKind::Function(function) => {
                    if let Some(implementation) = self.collect_local_def(
                        module_id,
                        item,
                        source,
                        ScopeBindingProvenance::Direct,
                    ) {
                        self.collect_proc_macro_definition(
                            module_id,
                            item,
                            source,
                            function,
                            implementation,
                            order,
                        );
                    }
                }
                ItemKind::Enum(enum_item) => {
                    self.collect_enum(module_id, item, source, enum_item);
                }
                ItemKind::Trait(trait_item) => {
                    if self
                        .collect_local_def(module_id, item, source, ScopeBindingProvenance::Direct)
                        .is_some()
                    {
                        self.collect_associated_macro_calls(
                            item_tree,
                            module_id,
                            source,
                            &trait_item.items,
                            &order,
                            &module_file_context,
                        );
                    }
                }
                _ => {
                    self.collect_local_def(module_id, item, source, ScopeBindingProvenance::Direct);
                }
            }
        }

        Ok(())
    }

    /// Exposes supported foreign declarations in the enclosing Rust module.
    ///
    /// For `extern "C" { fn read(); }`, `read` gets an ordinary module binding. Its separate block
    /// owner remains available so later phases can tell that the function is a declaration, not a
    /// Rust body that failed to lower.
    fn collect_extern_block(
        &mut self,
        item_tree: &ItemTreePackage,
        module_id: ModuleId,
        block_source: ItemTreeRef,
        extern_block: &ExternBlockItem,
    ) {
        for child_id in &extern_block.items {
            let child_source = ItemTreeRef {
                file_id: block_source.file_id,
                item: *child_id,
            };
            let child = item_tree
                .item(child_source)
                .expect("extern-block child should exist while collecting def map");
            if !self.is_item_enabled(child) {
                continue;
            }

            // Macro calls are retained by ItemTree for source fidelity, but this path never adds
            // them to the expansion queue. The ordinary local-def classifier rejects them here.
            if let Some(local_def) = self.collect_local_def(
                module_id,
                child,
                child_source,
                ScopeBindingProvenance::Direct,
            ) {
                self.def_map_builder
                    .insert_foreign_block(local_def, block_source.into());
            }
        }
    }

    fn is_item_enabled(&self, item: &ItemNode) -> bool {
        self.cfg_evaluator().is_enabled(&item.cfg)
    }

    /// Records one module-scope local definition and inserts its direct binding into the base scope.
    fn collect_local_def(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        provenance: ScopeBindingProvenance,
    ) -> Option<LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let namespaces = kind.scope_namespaces(&item.kind);
        let name = item.name.clone()?;
        let visibilities = self.def_map_builder.resolve_local_def_visibilities(
            module_id,
            &item.kind,
            &item.visibility,
        );

        let local_def_id = self.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            namespaces,
            visibility: item.visibility.clone(),
            source: source.into(),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.def_map_builder
            .module_mut(module_id)
            .expect("module should exist for collected local definition")
            .local_defs
            .push(local_def_id);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.crate_ref),
            local_def: local_def_id,
        });
        let scope = self
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for collected local definition");
        for namespace in namespaces.iter() {
            scope.insert_binding(
                &name,
                namespace,
                ScopeBinding::new(def, *visibilities.get(namespace), provenance),
            );
        }
        Some(local_def_id)
    }

    /// Records enum variants as value-namespace import targets without injecting them into the
    /// enclosing module scope. Variants become bare names only through explicit/prelude/glob
    /// imports, while `Enum::Variant` is handled by path resolution against this table.
    fn collect_enum(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        enum_item: &EnumItem,
    ) {
        let Some(local_def_id) =
            self.collect_local_def(module_id, item, source, ScopeBindingProvenance::Direct)
        else {
            return;
        };
        let visibility = self
            .def_map_builder
            .resolve_visibility(module_id, &item.visibility);

        self.def_map_builder.alloc_local_enum_variants(
            module_id,
            local_def_id,
            enum_item,
            visibility,
            item.file_id,
        );
    }

    /// Records a macro definition both as a normal macro-namespace binding and as macro payload
    /// that can be compiled later if a call resolves to it.
    fn collect_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        macro_definition: &MacroDefinitionItem,
        order: ItemOrder,
    ) {
        let provenance = match macro_definition {
            MacroDefinitionItem::MacroRules { attrs, .. }
                if self.macro_definition_is_exported(attrs) =>
            {
                ScopeBindingProvenance::DirectMacroExport
            }
            MacroDefinitionItem::MacroRules { .. } => ScopeBindingProvenance::DirectMacroRules,
            MacroDefinitionItem::MacroDef { .. } => ScopeBindingProvenance::Direct,
        };
        let Some(local_def_id) = self.collect_local_def(module_id, item, source, provenance) else {
            return;
        };

        self.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        if matches!(macro_definition, MacroDefinitionItem::MacroRules { .. })
            && let Some(name) = item.name.clone()
        {
            self.textual_macro_scopes
                .record_definition(module_id, name, local_def_id, order);
        }
        if let MacroDefinitionItem::MacroRules { attrs, .. } = macro_definition
            && self.macro_definition_is_exported(attrs)
            && let Some(name) = &item.name
        {
            self.export_macro_definition_to_root(name, local_def_id);
        }
        self.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_item(
                macro_definition,
                item.docs.clone(),
                self.edition,
                self.crate_ref,
            ),
        );
    }

    /// Adds the exported macro identity of an annotated function without confusing that identity
    /// with the function's ordinary value-namespace definition.
    fn collect_proc_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        function: &FunctionItem,
        implementation: LocalDefId,
        order: ItemOrder,
    ) {
        let Some(proc_macro) = &function.proc_macro else {
            return;
        };
        if !matches!(self.target_kind, TargetKind::ProcMacro) || Some(module_id) != self.root_module
        {
            return;
        }

        let local_def_id = self.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: proc_macro.name.clone(),
            kind: LocalDefKind::MacroDefinition,
            namespaces: NamespaceSet::MACROS,
            visibility: VisibilityLevel::Public,
            source: source.into(),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.def_map_builder
            .module_mut(module_id)
            .expect("proc-macro root module should exist")
            .local_defs
            .push(local_def_id);

        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.crate_ref),
            local_def: local_def_id,
        });
        self.base_scopes
            .get_mut(module_id.0)
            .expect("proc-macro root scope should exist")
            .insert_binding(
                &proc_macro.name,
                Namespace::Macros,
                ScopeBinding::new(def, Visibility::Public, ScopeBindingProvenance::Direct),
            );
        self.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        self.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_proc_macro(
                proc_macro.kind,
                implementation,
                item.docs.clone(),
                self.edition,
                self.crate_ref,
            ),
        );
    }

    /// Makes a `#[macro_export]` definition visible through the crate root macro namespace.
    fn export_macro_definition_to_root(&mut self, name: &Name, local_def_id: LocalDefId) {
        let root_module = self
            .root_module
            .expect("root module should exist before macro export collection");
        self.base_scopes
            .get_mut(root_module.0)
            .expect("root scope should exist before macro export collection")
            .insert_binding(
                name,
                Namespace::Macros,
                ScopeBinding::new(
                    DefId::Local(LocalDefRef {
                        origin: DefMapRef::Crate(self.crate_ref),
                        local_def: local_def_id,
                    }),
                    Visibility::Public,
                    ScopeBindingProvenance::MacroExport,
                ),
            );
    }

    fn macro_definition_is_exported(&self, attrs: &MacroDefinitionAttrs) -> bool {
        if attrs.macro_export {
            return true;
        }

        let cfg = self.cfg_evaluator();
        attrs
            .cfg_attr_macro_export
            .iter()
            .any(|predicate| cfg.is_predicate_enabled(predicate))
    }

    /// Keeps an item-position macro call and its logical filesystem base for later expansion.
    ///
    /// Expansion may happen after the source AST and ItemTree lowering context are gone. Retaining
    /// the call-site base here lets a generated `mod child;` resolve beside the caller rather than
    /// beside the macro definition.
    #[allow(clippy::too_many_arguments)]
    fn collect_macro_call(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        macro_call: &MacroCallItem,
        order: ItemOrder,
        placement: MacroCallPlacement,
        module_file_context: Arc<ModuleFileContext>,
    ) {
        self.macro_directives.push(MacroDirective {
            call: MacroCallSite {
                module: module_id,
                source,
                path: macro_call.path.clone(),
                callee: macro_call.callee.clone(),
                args: macro_call.args.clone(),
                builtin: macro_call.builtin.clone(),
                dollar_crate: None,
                file_id: item.file_id,
                span: item.span,
                order,
                placement,
                module_file_context,
            },
            origin: MacroCallOrigin::Source,
            state: MacroDirectiveState::Pending,
        });
    }

    /// Queues macros nested in a source trait or impl without exposing them as module items.
    fn collect_associated_macro_calls(
        &mut self,
        item_tree: &ItemTreePackage,
        module_id: ModuleId,
        parent_source: ItemTreeRef,
        item_ids: &[ItemTreeId],
        order: &ItemOrder,
        module_file_context: &Arc<ModuleFileContext>,
    ) {
        for (index, item_id) in item_ids.iter().copied().enumerate() {
            let source = ItemTreeRef {
                file_id: parent_source.file_id,
                item: item_id,
            };
            let Some(item) = item_tree.item(source) else {
                continue;
            };
            if !self.is_item_enabled(item) {
                continue;
            }
            let ItemKind::MacroCall(macro_call) = &item.kind else {
                continue;
            };
            self.collect_macro_call(
                module_id,
                item,
                source,
                macro_call,
                order.generated_child(index),
                MacroCallPlacement::AssociatedItems {
                    call_source: source.into(),
                },
                Arc::clone(module_file_context),
            );
        }
    }

    /// Records one module-scope impl block without inserting a namespace binding.
    fn collect_local_impl(&mut self, module_id: ModuleId, item: &ItemNode, source: ItemTreeRef) {
        let local_impl_id = self.def_map_builder.alloc_local_impl(LocalImplData {
            module: module_id,
            source: source.into(),
            file_id: item.file_id,
            span: item.span,
        });
        self.def_map_builder
            .module_mut(module_id)
            .expect("module should exist for collected impl block")
            .impls
            .push(local_impl_id);
    }

    /// Creates a child module node and recursively walks its item source when available.
    fn collect_module(
        &mut self,
        item_tree: &ItemTreePackage,
        parent_module: ModuleId,
        item: &ItemNode,
        module_item: &ModuleItem,
        order: ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
    ) -> anyhow::Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        let source = &module_item.source;
        let resolved_file = match source {
            ModuleSource::Inline { .. } => None,
            ModuleSource::OutOfLine => self.known_module_files.resolve(
                &module_file_context,
                module_name.as_str(),
                module_item.path_override.as_deref(),
            ),
        };
        let origin = match source {
            ModuleSource::Inline { .. } => ModuleOrigin::Inline {
                declaration_file: item.file_id,
                declaration_span: item.span,
            },
            ModuleSource::OutOfLine => ModuleOrigin::OutOfLine {
                declaration_file: item.file_id,
                declaration_span: item.span,
                definition_file: resolved_file.as_ref().map(|(file_id, _)| *file_id),
                file_selection: ModuleFileSelection::from_path_override(
                    module_item.path_override.as_deref(),
                ),
            },
        };

        let inner_docs = match source {
            ModuleSource::Inline { .. } => module_item.inner_docs.clone(),
            ModuleSource::OutOfLine => match resolved_file.as_ref() {
                Some((definition_file, _)) => item_tree
                    .file(*definition_file)
                    .with_context(|| {
                        format!(
                            "while attempting to fetch out-of-line module docs for {:?}",
                            definition_file
                        )
                    })?
                    .docs
                    .clone(),
                None => None,
            },
        };
        let semantic_visibility = self
            .def_map_builder
            .resolve_visibility(parent_module, &item.visibility);
        let child_module = self.alloc_module(
            Some(parent_module),
            Some(module_name.clone()),
            item.name_span,
            Documentation::concat(item.docs.clone(), inner_docs),
            item.user_facing_attrs,
            semantic_visibility,
            origin,
        );
        self.link_child_module(
            parent_module,
            child_module,
            &module_name,
            semantic_visibility,
        );
        self.textual_macro_scopes
            .record_module_declaration(child_module, order.clone());

        match source {
            ModuleSource::Inline { items } => {
                // Inline modules already carry their lowered items inside the parent file tree.
                let child_context = Arc::new(
                    module_file_context
                        .descend_inline(module_name.as_str(), module_item.path_override.as_deref()),
                );
                self.collect_items_in_context(
                    item_tree,
                    child_module,
                    item.file_id,
                    items,
                    child_context,
                )
                .context("while attempting to collect inline module items")?;
            }
            ModuleSource::OutOfLine => {
                let Some((definition_file, child_context)) = resolved_file else {
                    return Ok(());
                };
                // Allocate the declaration above, but do not re-enter a file already on this
                // source path. The same completed file may still be interpreted elsewhere later.
                if !self.active_files.insert(definition_file) {
                    return Ok(());
                }
                // Out-of-line modules point at another lowered file tree.
                let file_tree = item_tree.file(definition_file).with_context(|| {
                    format!(
                        "while attempting to fetch out-of-line module item tree for {:?}",
                        definition_file
                    )
                })?;
                let collected = self.collect_items_in_context(
                    item_tree,
                    child_module,
                    definition_file,
                    &file_tree.top_level,
                    child_context,
                );
                self.active_files.remove(&definition_file);
                collected.context("while attempting to collect out-of-line module items")?;
            }
        }
        if let Some(macro_use) = &module_item.macro_use
            && let Some(selector) = macro_use.active_selector(&self.cfg_evaluator())
        {
            self.textual_macro_scopes.import_module_definitions(
                parent_module,
                child_module,
                order,
                &selector,
            );
        }

        Ok(())
    }

    /// Links a child module into its parent's module tree and type namespace.
    fn link_child_module(
        &mut self,
        parent_module: ModuleId,
        child_module: ModuleId,
        module_name: &Name,
        visibility: Visibility,
    ) {
        self.def_map_builder
            .module_mut(parent_module)
            .expect("parent module should exist for child link")
            .children
            .push((module_name.clone(), child_module));
        self.base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for child link")
            .insert_binding(
                module_name,
                Namespace::Types,
                ScopeBinding::new(
                    DefId::Module(ModuleRef {
                        origin: DefMapRef::Crate(self.crate_ref),
                        module: child_module,
                    }),
                    visibility,
                    ScopeBindingProvenance::Direct,
                ),
            );
    }

    /// Records raw import directives for later fixed-point resolution.
    ///
    /// This phase only normalizes the path and binding metadata. It does not try to resolve the
    /// import yet.
    fn collect_use(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        use_item: &UseItem,
    ) {
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let Some(path) = ImportPath::from_use_path(&import.path, None) else {
                continue;
            };
            // Imports like `use foo::{self};` strip the trailing `self`. If nothing remains, there
            // is no path to record here.
            if path.semantic().is_empty() {
                continue;
            }
            let visibility = self
                .def_map_builder
                .resolve_visibility(module_id, &item.visibility);

            let import_id = self.def_map_builder.alloc_import(ImportData {
                module: module_id,
                visibility,
                is_reexport: ImportData::source_visibility_reexports(&item.visibility),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    rg_item_tree::ImportAlias::Explicit { span, .. } => Some(*span),
                    rg_item_tree::ImportAlias::Inferred | rg_item_tree::ImportAlias::Hidden => None,
                },
                source: source.into(),
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.def_map_builder
                .module_mut(module_id)
                .expect("module should exist for lowered import")
                .imports
                .push(import_id);
        }
    }

    /// Lowers `extern crate` into an immediate type-namespace binding.
    ///
    /// Unlike normal `use`, this can be bound during collection because the crate roots are
    /// already known.
    fn collect_extern_crate(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        extern_crate: &ExternCrateItem,
    ) {
        let Some(extern_name) = extern_crate.name.clone() else {
            return;
        };

        let module_ref = if extern_name == "self" {
            ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: self
                    .root_module
                    .expect("root module should exist before extern crate collection"),
            }
        } else {
            let Some(module_ref) = self.extern_prelude.extern_crate_source(&extern_name) else {
                return;
            };
            module_ref
        };

        if let Some(macro_use) = &extern_crate.macro_use
            && let Some(selector) = macro_use.active_selector(&self.cfg_evaluator())
        {
            // `extern crate dep as _` hides the type binding but still imports macros. Record the
            // macro-use bridge before resolving the optional binding name.
            self.macro_use_imports.push(MacroUseImport {
                module: module_id,
                source_module: module_ref,
                selector,
            });
        }

        let Some(binding_name) =
            ImportBinding::from_alias(&extern_crate.alias).resolve(Some(extern_name.clone()))
        else {
            return;
        };
        let visibility = self
            .def_map_builder
            .resolve_visibility(module_id, &item.visibility);

        // Crate-root declarations also enter Rust's crate-wide extern prelude. Keep the direct
        // root binding below as well: it retains the declaration's source scope and provenance.
        if self.root_module == Some(module_id) {
            self.extern_prelude
                .insert_explicit_alias(binding_name.clone(), module_ref);
        }

        // `extern crate` contributes directly to the base scope rather than through a deferred
        // import record.
        self.base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for extern crate binding")
            .insert_binding(
                &binding_name,
                Namespace::Types,
                ScopeBinding::new(
                    DefId::Module(module_ref),
                    visibility,
                    ScopeBindingProvenance::ExternCrate,
                ),
            );
    }

    fn cfg_evaluator(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(self.cfg_options, self.target_kind.enables_test_cfg())
    }
}
