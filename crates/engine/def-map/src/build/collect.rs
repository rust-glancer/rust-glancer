//! Collects the unresolved def-map skeleton from item trees.
//!
//! This phase walks one target's module tree and records only what is directly visible from the
//! syntax:
//! - module hierarchy
//! - module-scope local definitions
//! - raw import directives
//! - immediate bindings such as child modules and `extern crate`
//!
//! Import resolution itself happens during build finalization, using the `base_scopes` produced
//! here.

use std::collections::HashMap;

use anyhow::Context as _;

use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::{DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef, TargetRef};
use rg_ir_storage::{
    DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
    LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, ModuleData, ModuleOrigin,
    ModuleScope, ModuleScopeBuilder, Namespace, ScopeBinding, ScopeBindingOrigin,
};
use rg_item_tree::{
    Documentation, ExternCrateItem, ItemKind, ItemNode, ItemTreeDb, ItemTreeId, ItemTreeRef,
    MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem, MacroUseAttr, MacroUseSelector,
    ModuleItem, ModuleSource, Package as ItemTreePackage, UseImport, UseItem, VisibilityLevel,
};
use rg_parse::{Package, Target};
use rg_text::Name;
use rg_workspace::{RustEdition, TargetKind};

use crate::PackageSlot;

use super::macros::{
    ItemOrder, MacroCallSite, MacroDefinitionRecord, MacroDirective, MacroDirectiveState,
    MacroUseImport, TextualMacroScopes,
};

/// Collected state for one target before fixed-point import resolution.
///
/// `def_map` contains the frozen structural data, while `base_scopes` keeps the directly known
/// bindings that later passes start from.
pub(super) struct TargetState {
    pub(super) target: TargetRef,
    pub(super) target_name: String,
    pub(super) root_module: ModuleId,
    pub(super) edition: RustEdition,
    /// Target-specific cfg values used to decide which collected items really exist.
    pub(super) cfg_options: CfgOptions,
    pub(super) target_kind: TargetKind,
    pub(super) def_map_builder: DefMapBuilder,
    pub(super) base_scopes: Vec<ModuleScopeBuilder>,
    pub(super) implicit_roots: HashMap<Name, ModuleRef>,
    pub(super) prelude: Option<ModuleRef>,
    pub(super) macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    pub(super) textual_macro_scopes: TextualMacroScopes,
    pub(super) macro_use_imports: Vec<MacroUseImport>,
    pub(super) macro_directives: Vec<MacroDirective>,
}

impl TargetState {
    pub(super) fn push_macro_call(&mut self, call: MacroCallSite) {
        self.macro_directives.push(MacroDirective {
            call,
            state: MacroDirectiveState::Pending,
        });
    }

    pub(super) fn cfg_evaluator(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(
            &self.cfg_options,
            matches!(self.target_kind, TargetKind::Test | TargetKind::Bench),
        )
    }
}

/// Collects unresolved target states for every package/target pair.
///
/// The nested return shape mirrors the parsed package/target layout so later resolution can move
/// between targets by package slot and target slot.
pub(super) fn collect_target_states(
    packages: &[Package],
    item_tree: &ItemTreeDb,
    implicit_roots: &[Vec<HashMap<Name, ModuleRef>>],
) -> anyhow::Result<Vec<Vec<TargetState>>> {
    let mut states = Vec::with_capacity(packages.len());

    for (package_slot, package) in packages.iter().enumerate() {
        let item_tree_package = item_tree.package(package_slot).with_context(|| {
            format!(
                "while attempting to fetch item tree package for {}",
                package.package_name()
            )
        })?;
        states.push(collect_package_target_states(
            package_slot,
            package,
            item_tree_package,
            implicit_roots,
        )?);
    }

    Ok(states)
}

pub(super) fn collect_package_target_states(
    package_slot: usize,
    package: &Package,
    item_tree_package: &ItemTreePackage,
    implicit_roots: &[Vec<HashMap<Name, ModuleRef>>],
) -> anyhow::Result<Vec<TargetState>> {
    let mut package_states = Vec::with_capacity(package.targets().len());

    for target in package.targets() {
        let target_ref = TargetRef {
            package: PackageSlot(package_slot),
            target: target.id,
        };
        let target_roots = implicit_roots
            .get(package_slot)
            .and_then(|package_roots| package_roots.get(target.id.0))
            .expect("implicit roots should exist for every parsed target");
        let target_root = item_tree_package.target_root(target.id).with_context(|| {
            format!(
                "while attempting to fetch item tree target root for {}",
                target.name
            )
        })?;

        let collector = TargetScopeCollector::new(
            target_ref,
            package.edition(),
            package.cfg_options(),
            target.kind.clone(),
            target_roots,
        );
        let state = collector
            .collect(item_tree_package, target, target_root.root_file)
            .with_context(|| {
                format!(
                    "while attempting to collect target scope for {}",
                    target.name
                )
            })?;
        package_states.push(state);
    }

    Ok(package_states)
}

/// Mutable collector for one target's module tree.
///
/// The collector builds two parallel structures:
/// - `def_map.modules`, which is the final structural payload
/// - `base_scopes`, which starts with only directly known bindings and is enriched later
struct TargetScopeCollector<'db> {
    target: TargetRef,
    edition: RustEdition,
    cfg_options: &'db CfgOptions,
    target_kind: TargetKind,
    implicit_roots: &'db HashMap<Name, ModuleRef>,
    root_module: Option<ModuleId>,
    def_map_builder: DefMapBuilder,
    base_scopes: Vec<ModuleScopeBuilder>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionRecord>,
    textual_macro_scopes: TextualMacroScopes,
    macro_use_imports: Vec<MacroUseImport>,
    macro_directives: Vec<MacroDirective>,
}

impl<'db> TargetScopeCollector<'db> {
    fn new(
        target: TargetRef,
        edition: RustEdition,
        cfg_options: &'db CfgOptions,
        target_kind: TargetKind,
        implicit_roots: &'db HashMap<Name, ModuleRef>,
    ) -> Self {
        Self {
            target,
            edition,
            cfg_options,
            target_kind,
            implicit_roots,
            root_module: None,
            def_map_builder: DefMapBuilder::new(target),
            base_scopes: Vec::new(),
            macro_definitions: HashMap::new(),
            textual_macro_scopes: TextualMacroScopes::default(),
            macro_use_imports: Vec::new(),
            macro_directives: Vec::new(),
        }
    }

    /// Walks the target starting from its root file and returns the unresolved target state.
    fn collect(
        mut self,
        item_tree: &ItemTreePackage,
        target: &Target,
        root_file: rg_parse::FileId,
    ) -> anyhow::Result<TargetState> {
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
            ModuleOrigin::Root {
                file_id: target.root_file,
            },
        );
        self.root_module = Some(root_module);

        self.collect_items(item_tree, root_module, root_file, &root_file_tree.top_level)
            .context("while attempting to collect root file items")?;

        Ok(TargetState {
            target: self.target,
            target_name: target.name.clone(),
            root_module,
            edition: self.edition,
            cfg_options: self.cfg_options.clone(),
            target_kind: self.target_kind.clone(),
            def_map_builder: self.def_map_builder,
            base_scopes: self.base_scopes,
            implicit_roots: self.implicit_roots.clone(),
            prelude: None,
            macro_definitions: self.macro_definitions,
            textual_macro_scopes: self.textual_macro_scopes,
            macro_use_imports: self.macro_use_imports,
            macro_directives: self.macro_directives,
        })
    }

    /// Allocates one module in both the def-map payload and the base-scope table.
    fn alloc_module(
        &mut self,
        parent: Option<ModuleId>,
        name: Option<Name>,
        name_span: Option<rg_parse::Span>,
        docs: Option<rg_item_tree::Documentation>,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let module_id = self.def_map_builder.alloc_module(ModuleData {
            name,
            name_span,
            docs,
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

    /// Walks one module's items and records everything that is immediately knowable.
    fn collect_items(
        &mut self,
        item_tree: &ItemTreePackage,
        module_id: ModuleId,
        file_id: rg_parse::FileId,
        items: &[ItemTreeId],
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
                ItemKind::ExternCrate(extern_crate) => {
                    self.collect_extern_crate(module_id, item, extern_crate);
                }
                ItemKind::Module(module_item) => {
                    self.collect_module(item_tree, module_id, item, module_item, order)
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
                ItemKind::Impl(_) => self.collect_local_impl(module_id, item, source),
                ItemKind::MacroCall(macro_call) => {
                    self.collect_macro_call(module_id, item, source, macro_call, order);
                }
                ItemKind::MacroDefinition(macro_definition) => {
                    self.collect_macro_definition(module_id, item, source, macro_definition, order);
                }
                _ => {
                    self.collect_local_def(module_id, item, source);
                }
            }
        }

        Ok(())
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
    ) -> Option<LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let namespace = kind.namespace();
        let name = item.name.clone()?;

        let local_def_id = self.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            visibility: item.visibility.clone(),
            source: source.into(),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
        });
        self.def_map_builder
            .module_mut(module_id)
            .expect("module should exist for collected local definition")
            .local_defs
            .push(local_def_id);
        self.base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for collected local definition")
            .insert_binding(
                &name,
                namespace,
                ScopeBinding {
                    def: DefId::Local(LocalDefRef {
                        origin: DefMapRef::Target(self.target),
                        local_def: local_def_id,
                    }),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        origin: DefMapRef::Target(self.target),
                        module: module_id,
                    },
                    origin: ScopeBindingOrigin::Direct,
                },
            );
        Some(local_def_id)
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
        let Some(local_def_id) = self.collect_local_def(module_id, item, source) else {
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
            MacroDefinitionData::from_item(macro_definition, self.edition, self.target),
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
                ScopeBinding {
                    def: DefId::Local(LocalDefRef {
                        origin: DefMapRef::Target(self.target),
                        local_def: local_def_id,
                    }),
                    visibility: VisibilityLevel::Public,
                    owner: ModuleRef {
                        origin: DefMapRef::Target(self.target),
                        module: root_module,
                    },
                    origin: ScopeBindingOrigin::MacroExport,
                },
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

    /// Keeps item-position macro calls for the later def-map expansion loop.
    fn collect_macro_call(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        macro_call: &MacroCallItem,
        order: ItemOrder,
    ) {
        self.macro_directives.push(MacroDirective {
            call: MacroCallSite {
                module: module_id,
                source,
                path: macro_call.path.clone(),
                callee: macro_call.callee.clone(),
                args: macro_call.args.clone(),
                builtin: macro_call.builtin.clone(),
                dollar_crate_target: None,
                file_id: item.file_id,
                span: item.span,
                order,
            },
            state: MacroDirectiveState::Pending,
        });
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
    ) -> anyhow::Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        let source = &module_item.source;
        let origin = match source {
            ModuleSource::Inline { .. } => ModuleOrigin::Inline {
                declaration_file: item.file_id,
                declaration_span: item.span,
            },
            ModuleSource::OutOfLine { definition_file } => ModuleOrigin::OutOfLine {
                declaration_file: item.file_id,
                declaration_span: item.span,
                definition_file: *definition_file,
            },
        };

        let inner_docs = match source {
            ModuleSource::Inline { .. } => module_item.inner_docs.clone(),
            ModuleSource::OutOfLine {
                definition_file: Some(definition_file),
            } => item_tree
                .file(*definition_file)
                .with_context(|| {
                    format!(
                        "while attempting to fetch out-of-line module docs for {:?}",
                        definition_file
                    )
                })?
                .docs
                .clone(),
            ModuleSource::OutOfLine {
                definition_file: None,
            } => None,
        };
        let child_module = self.alloc_module(
            Some(parent_module),
            Some(module_name.clone()),
            item.name_span,
            Documentation::concat(item.docs.clone(), inner_docs),
            origin,
        );
        self.link_child_module(
            parent_module,
            child_module,
            &module_name,
            item.visibility.clone(),
        );
        self.textual_macro_scopes
            .record_module_declaration(child_module, order.clone());

        match source {
            ModuleSource::Inline { items } => {
                // Inline modules already carry their lowered items inside the parent file tree.
                self.collect_items(item_tree, child_module, item.file_id, items)
                    .context("while attempting to collect inline module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: Some(definition_file),
            } => {
                // Out-of-line modules point at another lowered file tree.
                let file_tree = item_tree.file(*definition_file).with_context(|| {
                    format!(
                        "while attempting to fetch out-of-line module item tree for {:?}",
                        definition_file
                    )
                })?;
                self.collect_items(
                    item_tree,
                    child_module,
                    *definition_file,
                    &file_tree.top_level,
                )
                .context("while attempting to collect out-of-line module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: None,
            } => {}
        }
        if let Some(macro_use) = &module_item.macro_use
            && let Some(selector) = self.active_macro_use_selector(macro_use)
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
        visibility: rg_item_tree::VisibilityLevel,
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
                ScopeBinding {
                    def: DefId::Module(ModuleRef {
                        origin: DefMapRef::Target(self.target),
                        module: child_module,
                    }),
                    visibility,
                    owner: ModuleRef {
                        origin: DefMapRef::Target(self.target),
                        module: parent_module,
                    },
                    origin: ScopeBindingOrigin::Direct,
                },
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
            let path = ImportPath::from_use_path(&import.path);
            // Imports like `use foo::{self};` strip the trailing `self`. If nothing remains, there
            // is no path to record here.
            if path.segments.is_empty() {
                continue;
            }

            let import_id = self.def_map_builder.alloc_import(ImportData {
                module: module_id,
                visibility: item.visibility.clone(),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                source_path: ImportSourcePath::from_use_path(&import.path),
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    rg_item_tree::ImportAlias::Explicit { span, .. } => Some(*span),
                    rg_item_tree::ImportAlias::Inferred | rg_item_tree::ImportAlias::Hidden => None,
                },
                source: source.into(),
                import_index,
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
    /// Unlike normal `use`, this can be bound during collection because the target roots are
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
                origin: DefMapRef::Target(self.target),
                module: self
                    .root_module
                    .expect("root module should exist before extern crate collection"),
            }
        } else {
            let Some(module_ref) = self.implicit_roots.get(&extern_name).copied() else {
                return;
            };
            module_ref
        };

        if let Some(macro_use) = &extern_crate.macro_use
            && let Some(selector) = self.active_macro_use_selector(macro_use)
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

        // `extern crate` contributes directly to the base scope rather than through a deferred
        // import record.
        self.base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for extern crate binding")
            .insert_binding(
                &binding_name,
                Namespace::Types,
                ScopeBinding {
                    def: DefId::Module(module_ref),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        origin: DefMapRef::Target(self.target),
                        module: module_id,
                    },
                    origin: ScopeBindingOrigin::Direct,
                },
            );
    }

    fn active_macro_use_selector(&self, attr: &MacroUseAttr) -> Option<MacroUseSelector> {
        let mut selector = attr.direct.clone();
        let cfg = self.cfg_evaluator();

        for cfg_attr in &attr.cfg_attr_macro_use {
            if !cfg.is_predicate_enabled(&cfg_attr.predicate) {
                continue;
            }
            match &mut selector {
                Some(selector) => selector.merge(&cfg_attr.selector),
                None => selector = Some(cfg_attr.selector.clone()),
            }
        }

        selector
    }

    fn cfg_evaluator(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(
            self.cfg_options,
            matches!(self.target_kind, TargetKind::Test | TargetKind::Bench),
        )
    }
}
