//! Collects already-lowered source files and source-like builtin payloads into def-map state.
//!
//! ItemTree lowers supported builtin payloads ahead of time and incrementally lowers files found
//! through macro source-file requests. For example, direct and macro-generated `include!` calls
//! both insert a real file into the caller's module, while a macro-generated `mod child;` inserts a
//! real file into an already allocated child module. This collector handles all of those forms so
//! they reuse real `ItemTreeRef`s, file-relative module resolution, impl lowering, extern crates,
//! and macro-use behavior instead of taking a synthetic generated-item path.

use std::{collections::HashSet, sync::Arc};

use anyhow::{Context as _, Result};

use crate::{
    ImportBinding, ImportData, ImportKind, ImportPath, LocalDefData, LocalDefKind, LocalImplData,
    MacroDefinitionData, ModuleData, ModuleFileSelection, ModuleOrigin, ModuleScope, Namespace,
    ScopeBinding, ScopeBindingProvenance, Visibility,
};
use rg_ir_model::{DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef};
use rg_item_tree::{
    Documentation, ExternBlockItem, ExternCrateItem, ImportAlias, ItemKind, ItemTreeId,
    ItemTreeRef, MacroDefinitionAttrs, MacroDefinitionItem, ModuleItem, ModuleSource,
    Package as ItemTreePackage, UseImport, UseItem, UserFacingAttrs,
};
use rg_parse::{FileId, ModuleFileContext};
use rg_text::Name;

use crate::build::{collect::CrateState, finalize::ScopeMatrix, macros::MacroExpansionApplyResult};

use super::{
    ItemOrder, MacroCallOrigin, MacroCallSite, MacroDefinitionRecord, MacroDirective,
    MacroDirectiveState, MacroUseImport,
};

/// Semantic module, ordering, and file context used to insert already-lowered items.
///
/// Builtin fragments use the macro call's position. A real module file starts in its already
/// allocated child module and gives its top-level items ordinary file order.
pub(super) struct SourceFragmentOrigin {
    pub(super) module: ModuleId,
    pub(super) order: ItemOrder,
    pub(super) parent_call: usize,
    pub(super) module_file_context: Arc<ModuleFileContext>,
}

/// Collector for ItemTree nodes introduced after the initial crate-scope walk.
///
/// Source-like builtins enter the caller's module, while a late module file enters an allocated
/// child module. Both keep their real source refs and use ordinary item collection behavior. The
/// distinction comes from [`SourceFragmentOrigin`]; it is not inferred from the physical filename.
pub(super) struct SourceFragmentCollector<'a> {
    pub(super) state: &'a mut CrateState,
    pub(super) current_scopes: &'a mut ScopeMatrix,
    pub(super) item_tree: &'a ItemTreePackage,
    pub(super) origin: SourceFragmentOrigin,
    pub(super) result: MacroExpansionApplyResult,
    pub(super) active_files: HashSet<FileId>,
}

impl SourceFragmentCollector<'_> {
    pub(super) fn collect_file(mut self, file_id: FileId) -> Result<MacroExpansionApplyResult> {
        let file_tree = self.item_tree.file(file_id).with_context(|| {
            format!("while attempting to fetch source fragment item tree for {file_id:?}")
        })?;

        // `include!` inserts the referenced file at the call site. Top-level items therefore
        // belong to the caller's module, but their source refs and spans still point to `file_id`.
        let origin_order = self.origin.order.clone();
        let module_file_context = Arc::clone(&self.origin.module_file_context);
        self.collect_file_items(
            self.origin.module,
            file_id,
            &file_tree.top_level,
            |index| origin_order.generated_child(index),
            module_file_context,
        )?;
        Ok(self.result)
    }

    /// Collects a real file as the contents of an already allocated module.
    pub(super) fn collect_module_file(
        mut self,
        file_id: FileId,
    ) -> Result<MacroExpansionApplyResult> {
        let file_tree = self.item_tree.file(file_id).with_context(|| {
            format!("while attempting to fetch module item tree for {file_id:?}")
        })?;
        let module_file_context = Arc::clone(&self.origin.module_file_context);
        self.collect_file_items(
            self.origin.module,
            file_id,
            &file_tree.top_level,
            ItemOrder::real,
            module_file_context,
        )?;
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
        self.collect_file_items(
            self.origin.module,
            file_id,
            items,
            |index| origin_order.generated_child(index),
            Arc::clone(&self.origin.module_file_context),
        )?;
        Ok(self.result)
    }

    /// Walks one real file while stopping only cycles on the active source path.
    ///
    /// Completed files are removed from the set so another module interpretation can collect the
    /// same file under a different context.
    fn collect_file_items(
        &mut self,
        module_id: ModuleId,
        file_id: FileId,
        items: &[ItemTreeId],
        order_for: impl Fn(usize) -> ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        if !self.active_files.insert(file_id) {
            return Ok(());
        }
        let collected =
            self.collect_items(module_id, file_id, items, order_for, module_file_context);
        self.active_files.remove(&file_id);
        collected
    }

    fn collect_items(
        &mut self,
        module_id: ModuleId,
        file_id: FileId,
        items: &[ItemTreeId],
        order_for: impl Fn(usize) -> ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        for (item_index, item_id) in items.iter().enumerate() {
            self.collect_item(
                module_id,
                file_id,
                *item_id,
                order_for(item_index),
                Arc::clone(&module_file_context),
            )?;
        }
        Ok(())
    }

    fn collect_item(
        &mut self,
        module_id: ModuleId,
        file_id: FileId,
        item_id: ItemTreeId,
        order: ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
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
            ItemKind::ExternBlock(extern_block) => {
                self.collect_extern_block(module_id, source, extern_block);
            }
            ItemKind::ExternCrate(extern_crate) => {
                self.collect_extern_crate(module_id, item, extern_crate);
            }
            ItemKind::Module(module_item) => {
                self.collect_module(
                    module_id,
                    item,
                    module_item,
                    order,
                    Arc::clone(&module_file_context),
                )
                .with_context(|| {
                    format!(
                        "while attempting to collect source fragment module {}",
                        item.name.as_deref().unwrap_or("<unnamed>")
                    )
                })?;
            }
            ItemKind::Use(use_item) => self.collect_use(module_id, item, source, use_item),
            ItemKind::Enum(enum_item) => self.collect_enum(module_id, item, source, enum_item),
            ItemKind::Impl(_) => self.collect_local_impl(module_id, item, source),
            ItemKind::MacroCall(macro_call) => {
                self.collect_macro_call(
                    module_id,
                    item,
                    source,
                    macro_call,
                    order,
                    module_file_context,
                );
            }
            ItemKind::MacroDefinition(macro_definition) => {
                self.collect_macro_definition(module_id, item, source, macro_definition, order);
            }
            _ => {
                self.collect_local_def(module_id, item, source, ScopeBindingProvenance::Direct);
            }
        }

        Ok(())
    }

    fn collect_extern_block(
        &mut self,
        module_id: ModuleId,
        block_source: ItemTreeRef,
        extern_block: &ExternBlockItem,
    ) {
        for child_id in &extern_block.items {
            let child_source = ItemTreeRef {
                file_id: block_source.file_id,
                item: *child_id,
            };
            let child = self
                .item_tree
                .item(child_source)
                .expect("source-fragment extern child should exist while collecting def map");
            if !self.is_item_enabled(child) {
                continue;
            }
            // Retained nested macro calls are not queued by extern-block collection. The ordinary
            // local-def classifier rejects them while accepting the supported declarations.
            if let Some(local_def) = self.collect_local_def(
                module_id,
                child,
                child_source,
                ScopeBindingProvenance::Direct,
            ) {
                self.state
                    .def_map_builder
                    .insert_foreign_block(local_def, block_source.into());
            }
        }
    }

    fn is_item_enabled(&self, item: &rg_item_tree::ItemNode) -> bool {
        self.state.cfg_evaluator().is_enabled(&item.cfg)
    }

    fn collect_local_def(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
        provenance: ScopeBindingProvenance,
    ) -> Option<LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let namespaces = kind.scope_namespaces(&item.kind);
        let name = item.name.clone()?;
        let visibilities = self.state.def_map_builder.resolve_local_def_visibilities(
            module_id,
            &item.kind,
            &item.visibility,
        );

        // Local definitions become immediately visible in both the frozen def-map being built and
        // the mutable scope snapshot used by the macro expansion fixed-point loop.
        let local_def_id = self.state.def_map_builder.alloc_local_def(LocalDefData {
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
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for source fragment local definition")
            .local_defs
            .push(local_def_id);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.state.crate_ref),
            local_def: local_def_id,
        });
        let base_scope = self
            .state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for source fragment local definition");
        let current_scope = self
            .current_scopes
            .module_scope_mut(self.state.crate_ref, module_id)
            .expect("current scope should exist for source fragment local definition");
        for namespace in namespaces.iter() {
            let binding = ScopeBinding::new(def, *visibilities.get(namespace), provenance);
            base_scope.insert_binding(&name, namespace, binding.clone());
            current_scope.insert_binding(&name, namespace, binding);
        }

        Some(local_def_id)
    }

    /// Record source-fragment enum variants with the same namespace facts as ordinary source.
    fn collect_enum(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
        source: ItemTreeRef,
        enum_item: &rg_item_tree::EnumItem,
    ) {
        let Some(local_def_id) =
            self.collect_local_def(module_id, item, source, ScopeBindingProvenance::Direct)
        else {
            return;
        };
        let visibility = self
            .state
            .def_map_builder
            .resolve_visibility(module_id, &item.visibility);
        self.state.def_map_builder.alloc_local_enum_variants(
            module_id,
            local_def_id,
            enum_item,
            visibility,
            item.file_id,
        );
    }

    fn collect_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &rg_item_tree::ItemNode,
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
        self.state.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_item(
                macro_definition,
                item.docs.clone(),
                self.state.edition,
                self.state.crate_ref,
            ),
        );
    }

    fn export_macro_definition_to_root(&mut self, name: &Name, local_def_id: LocalDefId) {
        let root_module = self.state.root_module;
        let binding = ScopeBinding::new(
            DefId::Local(LocalDefRef {
                origin: DefMapRef::Crate(self.state.crate_ref),
                local_def: local_def_id,
            }),
            Visibility::Public,
            ScopeBindingProvenance::MacroExport,
        );

        self.state
            .base_scopes
            .get_mut(root_module.0)
            .expect("root scope should exist before source fragment macro export collection")
            .insert_binding(name, Namespace::Macros, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, root_module)
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
        module_file_context: Arc<ModuleFileContext>,
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
                dollar_crate: None,
                file_id: item.file_id,
                span: item.span,
                order,
                module_file_context,
            },
            origin: MacroCallOrigin::Generated {
                parent_call: self.origin.parent_call,
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
        let local_impl_id = self.state.def_map_builder.alloc_local_impl(LocalImplData {
            module: module_id,
            source: source.into(),
            file_id: item.file_id,
            span: item.span,
        });
        self.state
            .def_map_builder
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
        module_file_context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        // Modules declared inside a source-like fragment are real modules in the caller's module
        // tree. Their declaration span stays with the fragment item, which keeps navigation precise.
        let source = &module_item.source;
        let resolved_file = match source {
            ModuleSource::Inline { .. } => None,
            ModuleSource::OutOfLine => self.state.resolve_module_file(
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
                Some((definition_file, _)) => self
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
                None => None,
            },
        };
        let semantic_visibility = self
            .state
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
        self.state
            .textual_macro_scopes
            .record_module_declaration(child_module, order.clone());

        // Once the child module exists, its contents start their own source order. The declaration
        // keeps its position in the surrounding fragment or generated expansion.
        match source {
            ModuleSource::Inline { items } => {
                let child_context = Arc::new(
                    module_file_context
                        .descend_inline(module_name.as_str(), module_item.path_override.as_deref()),
                );
                self.collect_items(
                    child_module,
                    item.file_id,
                    items,
                    ItemOrder::real,
                    child_context,
                )
                .context("while attempting to collect source fragment inline module items")?;
            }
            ModuleSource::OutOfLine => {
                let Some((definition_file, child_context)) = resolved_file else {
                    return Ok(());
                };
                let file_tree = self.item_tree.file(definition_file).with_context(|| {
                    format!(
                        "while attempting to fetch source fragment out-of-line module item tree for {:?}",
                        definition_file
                    )
                })?;
                self.collect_file_items(
                    child_module,
                    definition_file,
                    &file_tree.top_level,
                    ItemOrder::real,
                    child_context,
                )
                .context("while attempting to collect source fragment out-of-line module items")?;
            }
        }

        // Legacy `#[macro_use] mod child` makes child macro_rules definitions textually available
        // in the parent at the module declaration position.
        if let Some(macro_use) = &module_item.macro_use
            && let Some(selector) = macro_use.active_selector(&self.state.cfg_evaluator())
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

    #[allow(clippy::too_many_arguments)]
    fn alloc_module(
        &mut self,
        parent: Option<ModuleId>,
        name: Option<Name>,
        name_span: Option<rg_parse::Span>,
        docs: Option<Documentation>,
        user_facing_attrs: UserFacingAttrs,
        visibility: Visibility,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let module_id = self.state.def_map_builder.alloc_module(ModuleData {
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
        self.state.base_scopes.push(Default::default());
        self.current_scopes
            .push_module_scope(self.state.crate_ref, Default::default())
            .expect("current scopes should have a crate slot for source fragment module");
        module_id
    }

    fn link_child_module(
        &mut self,
        parent_module: ModuleId,
        child_module: ModuleId,
        module_name: &Name,
        visibility: Visibility,
    ) {
        self.state
            .def_map_builder
            .module_mut(parent_module)
            .expect("parent module should exist for source fragment child link")
            .children
            .push((module_name.clone(), child_module));
        let binding = ScopeBinding::new(
            DefId::Module(ModuleRef {
                origin: DefMapRef::Crate(self.state.crate_ref),
                module: child_module,
            }),
            visibility,
            ScopeBindingProvenance::Direct,
        );
        self.state
            .base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for source fragment child link")
            .insert_binding(module_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, parent_module)
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
            let Some(path) = ImportPath::from_use_path(&import.path, None) else {
                continue;
            };
            if path.semantic().is_empty() {
                continue;
            }

            let visibility = self
                .state
                .def_map_builder
                .resolve_visibility(module_id, &item.visibility);
            let import_id = self.state.def_map_builder.alloc_import(ImportData {
                module: module_id,
                visibility,
                kind: ImportKind::from_use_kind(import.kind),
                path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { span, .. } => Some(*span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: source.into(),
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.state
                .def_map_builder
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
                origin: DefMapRef::Crate(self.state.crate_ref),
                module: self.state.root_module,
            }
        } else {
            let Some(module_ref) = self.state.extern_prelude.extern_crate_source(&extern_name)
            else {
                return;
            };
            module_ref
        };

        if let Some(macro_use) = &extern_crate.macro_use
            && let Some(selector) = macro_use.active_selector(&self.state.cfg_evaluator())
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

        // Source-like builtin expansion behaves as if its items were written at the call site.
        // Therefore an expanded root declaration receives the same crate-wide alias behavior as
        // an ordinary root `extern crate`, while a declaration expanded in a child module does not.
        if module_id == self.state.root_module {
            self.state
                .extern_prelude
                .insert_explicit_alias(binding_name.clone(), module_ref);
        }

        let binding = ScopeBinding::new(
            DefId::Module(module_ref),
            self.state
                .def_map_builder
                .resolve_visibility(module_id, &item.visibility),
            ScopeBindingProvenance::ExternCrate,
        );
        self.state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for source fragment extern crate binding")
            .insert_binding(&binding_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, module_id)
            .expect("current scope should exist for source fragment extern crate binding")
            .insert_binding(&binding_name, Namespace::Types, binding);
    }
}
