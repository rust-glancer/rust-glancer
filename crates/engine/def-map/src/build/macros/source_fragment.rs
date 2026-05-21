//! Collects source-like builtin macro payloads into def-map state.
//!
//! Item-tree lowers supported builtin payloads ahead of time, so this collector can reuse real
//! `ItemTreeRef`s, file-relative module resolution, impl lowering, extern crates, and macro-use
//! handling instead of treating them as arbitrary generated syntax.

use anyhow::{Context as _, Result};

use rg_item_tree::{
    Documentation, ExternCrateItem, ImportAlias, ItemKind, ItemTreeId, ItemTreeRef,
    MacroDefinitionAttrs, MacroDefinitionItem, MacroUseAttr, MacroUseSelector, ModuleItem,
    ModuleSource, Package as ItemTreePackage, UseImport, UseItem, VisibilityLevel,
};
use rg_parse::FileId;
use rg_text::Name;

use crate::{
    DefId, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath, LocalDefData,
    LocalDefId, LocalDefKind, LocalDefRef, LocalImplData, ModuleData, ModuleId, ModuleOrigin,
    ModuleRef, ModuleScope, ScopeBinding, ScopeBindingOrigin,
    build::{collect::TargetState, finalize::ScopeMatrix, macros::MacroExpansionApplyResult},
    model::Namespace,
};

use super::{
    ItemOrder, MacroCallSite, MacroDefinitionRecord, MacroDirective, MacroDirectiveState,
    MacroUseImport,
};

/// Source-fragment collection starts at the macro call's module and textual order.
pub(super) struct SourceFragmentOrigin {
    pub(super) module: ModuleId,
    pub(super) order: ItemOrder,
}

/// Collector for item-tree nodes that should behave like ordinary source at the call site.
pub(super) struct SourceFragmentCollector<'a> {
    pub(super) state: &'a mut TargetState,
    pub(super) current_scopes: &'a mut ScopeMatrix,
    pub(super) item_tree: &'a ItemTreePackage,
    pub(super) origin: SourceFragmentOrigin,
    pub(super) result: MacroExpansionApplyResult,
}

impl SourceFragmentCollector<'_> {
    pub(super) fn collect_file(mut self, file_id: FileId) -> Result<MacroExpansionApplyResult> {
        let file_tree = self.item_tree.file(file_id).with_context(|| {
            format!("while attempting to fetch source fragment item tree for {file_id:?}")
        })?;

        // `include!` inserts the referenced file at the call site. Top-level items therefore
        // belong to the caller's module, but their source refs and spans still point to `file_id`.
        let origin_order = self.origin.order.clone();
        self.collect_items(self.origin.module, file_id, &file_tree.top_level, |index| {
            origin_order.generated_child(index)
        })?;
        Ok(self.result)
    }

    pub(super) fn collect_fragment(
        mut self,
        file_id: FileId,
        items: &[ItemTreeId],
    ) -> Result<MacroExpansionApplyResult> {
        // Source-like builtins such as `cfg_select!` lower their item payloads into the caller's
        // file tree ahead of time. Def-map only picks the active fragment and collects those item
        // ids at the macro call position.
        let origin_order = self.origin.order.clone();
        self.collect_items(self.origin.module, file_id, items, |index| {
            origin_order.generated_child(index)
        })?;
        Ok(self.result)
    }

    fn collect_items(
        &mut self,
        module_id: ModuleId,
        file_id: FileId,
        items: &[ItemTreeId],
        order_for: impl Fn(usize) -> ItemOrder,
    ) -> Result<()> {
        for (item_index, item_id) in items.iter().enumerate() {
            self.collect_item(module_id, file_id, *item_id, order_for(item_index))?;
        }
        Ok(())
    }

    fn collect_item(
        &mut self,
        module_id: ModuleId,
        file_id: FileId,
        item_id: ItemTreeId,
        order: ItemOrder,
    ) -> Result<()> {
        let source = ItemTreeRef {
            file_id,
            item: item_id,
        };
        let item = self
            .item_tree
            .item(source)
            .expect("source fragment item tree id should exist while collecting def map");
        if !self.is_item_enabled(item) {
            return Ok(());
        }
        self.result.mark_changed();

        // From this point on the collector mirrors ordinary item collection. The main difference
        // is that every allocated def keeps the fragment item's `ItemTreeRef` and source span.
        match &item.kind {
            ItemKind::ExternCrate(extern_crate) => {
                self.collect_extern_crate(module_id, item, extern_crate);
            }
            ItemKind::Module(module_item) => {
                self.collect_module(module_id, item, module_item, order)
                    .with_context(|| {
                        format!(
                            "while attempting to collect source fragment module {}",
                            item.name.as_deref().unwrap_or("<unnamed>")
                        )
                    })?;
            }
            ItemKind::Use(use_item) => self.collect_use(module_id, item, source, use_item),
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

        Ok(())
    }

    fn is_item_enabled(&self, item: &rg_item_tree::ItemNode) -> bool {
        self.state.cfg_evaluator().is_enabled(&item.cfg)
    }

    fn collect_local_def(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
    ) -> Option<LocalDefId> {
        let Some(kind) = LocalDefKind::from_item_tag(item.kind.tag()) else {
            return None;
        };
        let namespace = kind.namespace();
        let Some(name) = item.name.clone() else {
            return None;
        };

        // Local definitions become immediately visible in both the frozen def-map being built and
        // the mutable scope snapshot used by the macro expansion fixed-point loop.
        let local_def_id = self.state.def_map.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            visibility: item.visibility.clone(),
            source,
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
        });
        self.state
            .def_map
            .module_mut(module_id)
            .expect("module should exist for source fragment local definition")
            .local_defs
            .push(local_def_id);
        let binding = ScopeBinding {
            def: DefId::Local(LocalDefRef {
                target: self.state.target,
                local_def: local_def_id,
            }),
            visibility: item.visibility.clone(),
            owner: ModuleRef {
                target: self.state.target,
                module: module_id,
            },
            origin: ScopeBindingOrigin::Direct,
        };
        self.state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for source fragment local definition")
            .insert_binding(&name, namespace, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, module_id)
            .expect("current scope should exist for source fragment local definition")
            .insert_binding(&name, namespace, binding);

        Some(local_def_id)
    }

    fn collect_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
        macro_definition: &MacroDefinitionItem,
        order: ItemOrder,
    ) {
        let Some(local_def_id) = self.collect_local_def(module_id, item, source) else {
            return;
        };

        // A macro definition is also a normal local def, but expansion needs extra build-only
        // ordering and the serialized macro body for later calls to resolve and compile.
        self.state.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        if matches!(macro_definition, MacroDefinitionItem::MacroRules { .. })
            && let Some(name) = item.name.clone()
        {
            self.state
                .textual_macro_scopes
                .record_definition(module_id, name, local_def_id, order);
        }
        if let MacroDefinitionItem::MacroRules { attrs, .. } = macro_definition
            && self.macro_definition_is_exported(attrs)
            && let Some(name) = &item.name
        {
            self.export_macro_definition_to_root(name, local_def_id);
        }
        self.state.def_map.insert_macro_definition(
            local_def_id,
            crate::MacroDefinitionData::from_item(
                macro_definition,
                self.state.edition,
                self.state.target,
            ),
        );
    }

    fn export_macro_definition_to_root(&mut self, name: &Name, local_def_id: LocalDefId) {
        let Some(root_module) = self.state.def_map.root_module() else {
            return;
        };
        let binding = ScopeBinding {
            def: DefId::Local(LocalDefRef {
                target: self.state.target,
                local_def: local_def_id,
            }),
            visibility: VisibilityLevel::Public,
            owner: ModuleRef {
                target: self.state.target,
                module: root_module,
            },
            origin: ScopeBindingOrigin::MacroExport,
        };

        self.state
            .base_scopes
            .get_mut(root_module.0)
            .expect("root scope should exist before source fragment macro export collection")
            .insert_binding(name, Namespace::Macros, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, root_module)
            .expect("current root scope should exist for source fragment macro export")
            .insert_binding(name, Namespace::Macros, binding);
    }

    fn macro_definition_is_exported(&self, attrs: &MacroDefinitionAttrs) -> bool {
        if attrs.macro_export {
            return true;
        }

        let cfg = self.state.cfg_evaluator();
        attrs
            .cfg_attr_macro_export
            .iter()
            .any(|predicate| cfg.is_predicate_enabled(predicate))
    }

    fn collect_macro_call(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
        macro_call: &rg_item_tree::MacroCallItem,
        order: ItemOrder,
    ) {
        // Source-like fragments can contain further item-position macro calls. Queue them exactly
        // like source-file calls so later passes can resolve them against refreshed scopes.
        self.state.macro_directives.push(MacroDirective {
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

    fn collect_local_impl(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
    ) {
        let local_impl_id = self.state.def_map.alloc_local_impl(LocalImplData {
            module: module_id,
            source,
            file_id: item.file_id,
            span: item.span,
        });
        self.state
            .def_map
            .module_mut(module_id)
            .expect("module should exist for source fragment impl block")
            .impls
            .push(local_impl_id);
    }

    fn collect_module(
        &mut self,
        parent_module: ModuleId,
        item: &rg_item_tree::ItemNode,
        module_item: &ModuleItem,
        order: ItemOrder,
    ) -> Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        // Modules declared inside a source-like fragment are real modules in the caller's module
        // tree. Their declaration span stays with the fragment item, which keeps navigation precise.
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
            } => self
                .item_tree
                .file(*definition_file)
                .with_context(|| {
                    format!(
                        "while attempting to fetch source fragment out-of-line module docs for {:?}",
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
        self.state
            .textual_macro_scopes
            .record_module_declaration(child_module, order.clone());

        // Once the child module exists, collect its contents using the child file's own source
        // order. Only the module declaration itself is ordered as part of the include expansion.
        match source {
            ModuleSource::Inline { items } => {
                self.collect_items(child_module, item.file_id, items, ItemOrder::real)
                    .context("while attempting to collect source fragment inline module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: Some(definition_file),
            } => {
                let file_tree = self.item_tree.file(*definition_file).with_context(|| {
                    format!(
                        "while attempting to fetch source fragment out-of-line module item tree for {:?}",
                        definition_file
                    )
                })?;
                self.collect_items(
                    child_module,
                    *definition_file,
                    &file_tree.top_level,
                    ItemOrder::real,
                )
                .context("while attempting to collect source fragment out-of-line module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: None,
            } => {}
        }

        // Legacy `#[macro_use] mod child` makes child macro_rules definitions textually available
        // in the parent at the module declaration position.
        if let Some(macro_use) = &module_item.macro_use
            && let Some(selector) = self.active_macro_use_selector(macro_use)
        {
            self.state.textual_macro_scopes.import_module_definitions(
                parent_module,
                child_module,
                order,
                &selector,
            );
        }

        Ok(())
    }

    fn alloc_module(
        &mut self,
        parent: Option<ModuleId>,
        name: Option<Name>,
        name_span: Option<rg_parse::Span>,
        docs: Option<Documentation>,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let module_id = self.state.def_map.alloc_module(ModuleData {
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
        self.state.base_scopes.push(Default::default());
        self.current_scopes
            .push_module_scope(self.state.target, Default::default())
            .expect("current scopes should have a target slot for source fragment module");
        module_id
    }

    fn link_child_module(
        &mut self,
        parent_module: ModuleId,
        child_module: ModuleId,
        module_name: &Name,
        visibility: VisibilityLevel,
    ) {
        self.state
            .def_map
            .module_mut(parent_module)
            .expect("parent module should exist for source fragment child link")
            .children
            .push((module_name.clone(), child_module));
        let binding = ScopeBinding {
            def: DefId::Module(ModuleRef {
                target: self.state.target,
                module: child_module,
            }),
            visibility,
            owner: ModuleRef {
                target: self.state.target,
                module: parent_module,
            },
            origin: ScopeBindingOrigin::Direct,
        };
        self.state
            .base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for source fragment child link")
            .insert_binding(module_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, parent_module)
            .expect("current scope should exist for source fragment child link")
            .insert_binding(module_name, Namespace::Types, binding);
    }

    fn collect_use(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
        use_item: &UseItem,
    ) {
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let path = ImportPath::from_use_path(&import.path);
            if path.segments.is_empty() {
                continue;
            }

            let import_id = self.state.def_map.alloc_import(ImportData {
                module: module_id,
                visibility: item.visibility.clone(),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                source_path: ImportSourcePath::from_use_path(&import.path),
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { span, .. } => Some(*span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source,
                import_index,
            });
            self.state
                .def_map
                .module_mut(module_id)
                .expect("module should exist for source fragment import")
                .imports
                .push(import_id);
        }
    }

    fn collect_extern_crate(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        extern_crate: &ExternCrateItem,
    ) {
        let Some(extern_name) = extern_crate.name.clone() else {
            return;
        };

        // Macro-use imports do not require a type-namespace binding name. Record them before
        // applying aliases such as `extern crate dep as _`.
        let module_ref = if extern_name == "self" {
            ModuleRef {
                target: self.state.target,
                module: self.state.def_map.root_module().expect(
                    "root module should exist before source fragment extern crate collection",
                ),
            }
        } else {
            let Some(module_ref) = self.state.implicit_roots.get(&extern_name).copied() else {
                return;
            };
            module_ref
        };

        if let Some(macro_use) = &extern_crate.macro_use
            && let Some(selector) = self.active_macro_use_selector(macro_use)
        {
            self.state.macro_use_imports.push(MacroUseImport {
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

        let binding = ScopeBinding {
            def: DefId::Module(module_ref),
            visibility: item.visibility.clone(),
            owner: ModuleRef {
                target: self.state.target,
                module: module_id,
            },
            origin: ScopeBindingOrigin::Direct,
        };
        self.state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for source fragment extern crate binding")
            .insert_binding(&binding_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, module_id)
            .expect("current scope should exist for source fragment extern crate binding")
            .insert_binding(&binding_name, Namespace::Types, binding);
    }

    fn active_macro_use_selector(&self, attr: &MacroUseAttr) -> Option<MacroUseSelector> {
        // Merge direct `#[macro_use]` with active `#[cfg_attr(..., macro_use)]` selectors into the
        // single selector used by textual and extern-crate macro-use handling.
        let mut selector = attr.direct.clone();
        let cfg = self.state.cfg_evaluator();

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
}
