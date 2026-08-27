//! Collects syntax produced by macro expansion back into mutable crate state.
//!
//! Module-position definitions enter the macro call's module, while associated-item output is
//! retained as a replacement list for the invocation inside its trait or impl. In both cases item
//! payloads carry expansion spans where available, while generated imports and other
//! provenance-only facts may still point at the macro call site.
//!
//! Inline modules can be collected immediately. A generated `mod child;` instead emits a
//! project-owned source request and retains the continuation needed to allocate the module after
//! the real file has entered Parse and ItemTree. Generated `include!` calls take the same request
//! boundary through the macro-attempt path, then reuse this module's pending-source application
//! once their real file is available.

use std::sync::Arc;

use anyhow::{Context as _, Result};

use crate::{
    ImportBinding, ImportData, ImportKind, ImportPath, LocalDefData, LocalDefKind, LocalImplData,
    MacroDefinitionData, ModuleData, ModuleFileSelection, ModuleOrigin, ModuleScope, Namespace,
    ScopeBinding, ScopeBindingProvenance, Visibility,
};
use rg_ir_model::{CrateRef, DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef};
use rg_item_tree::{
    Documentation, ExternBlockItem, ImportAlias, ItemKind, ItemNode, ItemTreeDb, ItemTreeId,
    ItemTreeRef, MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem, ModuleItem,
    ModuleSource, UseImport, UseItem,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_parse::{FileId, ModuleFileContext, Span};
use rg_text::{Name, NameInterner, PackageNameInterners};

use crate::build::{
    MacroSourceFileResolution, MacroSourceFileResolutions, collect::CrateState,
    finalize::ScopeMatrix,
};
use crate::profile::metric;
use crate::{GeneratedItemRef, GeneratedSourceId, ItemSource, MacroSourceFileRequest};

use super::{
    ItemOrder, MacroCallOrigin, MacroCallPlacement, MacroCallSite, MacroDefinitionRecord,
    MacroExpansionApplyResult,
    generated_tree::GeneratedSourceLowering,
    source_fragment::{SourceFragmentCollector, SourceFragmentOrigin},
};

/// Call-site identity used for every item produced by one macro expansion.
///
/// Placement and module-file context also stay call-site-relative. Placement keeps associated
/// output in its trait or impl, while the file context makes an out-of-line child module belong
/// beside the invocation rather than the macro definition.
#[derive(Debug, Clone)]
pub(super) struct GeneratedOrigin {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
    pub(super) placement: MacroCallPlacement,
    pub(super) dollar_crate: Option<CrateRef>,
    pub(super) parent_call: usize,
    pub(super) module_file_context: Arc<ModuleFileContext>,
}

/// Continuation for one generated out-of-line module whose real file is project-owned.
///
/// The `mod` declaration is already retained in DefMap's synthetic macro-output arena. Resuming
/// therefore needs only that item id and the collection context that would have been used if the
/// real file resolution had been available immediately.
#[derive(Debug, Clone)]
pub(crate) struct PendingGeneratedModule {
    request: MacroSourceFileRequest,
    parent_module: ModuleId,
    generated_source: GeneratedSourceId,
    item_id: ItemTreeId,
    order: ItemOrder,
    module_file_context: Arc<ModuleFileContext>,
    origin: GeneratedOrigin,
}

/// Continuation for one builtin `include!` call produced by another macro.
///
/// ItemTree could not lower a payload for a call that did not exist in original source. The macro
/// directive and its call placement are already retained, so resuming only needs to splice the
/// project-lowered file into the module or associated owner and mark the directive complete.
#[derive(Debug, Clone)]
pub(crate) struct PendingGeneratedInclude {
    pub(super) request: MacroSourceFileRequest,
    pub(super) call_id: usize,
    pub(super) origin: GeneratedOrigin,
}

/// Collector that moves synthetic macro-output items into ordinary mutable DefMap state.
pub(super) struct GeneratedCollector<'a> {
    pub(super) state: &'a mut CrateState,
    pub(super) interner: &'a mut NameInterner,
    pub(super) current_scopes: &'a mut ScopeMatrix,
    pub(super) item_tree: &'a ItemTreeDb,
    pub(super) macro_source_file_resolutions: Option<&'a MacroSourceFileResolutions>,
    pub(super) origin: GeneratedOrigin,
    pub(super) result: MacroExpansionApplyResult,
}

impl GeneratedCollector<'_> {
    pub(super) fn collect_syntax(
        &mut self,
        expansion: ExpansionSyntax,
        macro_name: Option<&str>,
    ) -> Result<MacroExpansionApplyResult> {
        // Macro expansion has already run the parser over token trees. At this point we only check
        // syntax errors and splice item-shaped declarations from the generated root.
        let timer = metric::TIMING_PARSE_GENERATED_SOURCES.start_timer();
        let errors = expansion.parse.errors();
        timer.finish();
        if !errors.is_empty() {
            let macro_name = macro_name.unwrap_or("<unknown>");
            metric::MACRO_CALLS_FAILED.inc();
            metric::GENERATED_SOURCE_PARSE_FAILURES.inc();
            metric::FAILED_PARSE_BY_NAME.inc(macro_name);
            anyhow::bail!("macro expansion syntax has errors: {errors:?}");
        }
        metric::GENERATED_SOURCES_PARSED.inc();
        self.result.mark_changed();

        let generated_source = GeneratedSourceLowering::lower(
            &self.origin,
            expansion,
            self.interner,
            self.state.edition,
        )
        .context("while attempting to lower macro expansion into generated source")?;
        let generated_source_id = self
            .state
            .def_map_builder
            .alloc_generated_source(generated_source);
        let top_level = self
            .state
            .def_map_builder
            .partial()
            .generated_source(generated_source_id)
            .expect("generated source should exist immediately after allocation")
            .top_level
            .clone();

        // Generated items may introduce further macro calls. Module output enters ordinary
        // collection, while associated output is retained as a sparse replacement list for the
        // trait/impl call site that owns it.
        let timer = metric::TIMING_COLLECT_GENERATED_ITEMS.start_timer();
        match self.origin.placement {
            MacroCallPlacement::ModuleItems => {
                for (index, item_id) in top_level.into_iter().enumerate() {
                    self.collect_item(
                        self.origin.module,
                        generated_source_id,
                        item_id,
                        self.origin.order.generated_child(index),
                        Arc::clone(&self.origin.module_file_context),
                    )?;
                }
            }
            MacroCallPlacement::AssociatedItems { call_source } => {
                self.collect_associated_expansion(call_source, generated_source_id, &top_level);
            }
        }
        timer.finish();

        Ok(self.result)
    }

    fn collect_item(
        &mut self,
        module_id: ModuleId,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        order: ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        let item = self
            .state
            .def_map_builder
            .partial()
            .generated_source(generated_source)
            .and_then(|source| source.item(item_id))
            .expect("generated item id should exist while collecting def map")
            .clone();
        if !self.is_item_enabled(&item) {
            return Ok(());
        }
        metric::GENERATED_ITEMS_SEEN.inc();

        match &item.kind {
            ItemKind::MacroCall(macro_call) => {
                self.collect_macro_call(
                    module_id,
                    &item,
                    macro_call,
                    order,
                    MacroCallPlacement::ModuleItems,
                    module_file_context,
                );
            }
            ItemKind::MacroDefinition(macro_definition) => {
                self.collect_macro_definition(
                    module_id,
                    &item,
                    macro_definition,
                    generated_source,
                    item_id,
                    order,
                );
            }
            ItemKind::Module(module_item) => {
                self.collect_module(
                    module_id,
                    &item,
                    module_item,
                    generated_source,
                    item_id,
                    order,
                    module_file_context,
                )?;
            }
            ItemKind::Use(use_item) => self.collect_use(module_id, &item, use_item),
            ItemKind::Enum(enum_item) => {
                self.collect_enum(module_id, &item, enum_item, generated_source, item_id)
            }
            ItemKind::Impl(impl_item) => {
                self.collect_local_impl(module_id, &item, generated_source, item_id);
                self.collect_associated_macro_calls(
                    module_id,
                    generated_source,
                    &impl_item.items,
                    &order,
                    &module_file_context,
                );
            }
            ItemKind::ExternBlock(extern_block) => {
                self.collect_extern_block(module_id, extern_block, generated_source, item_id)
            }
            ItemKind::AsmExpr | ItemKind::ExternCrate(_) => {}
            ItemKind::Trait(trait_item) => {
                if self
                    .collect_named_def(
                        module_id,
                        &item,
                        generated_source,
                        item_id,
                        ScopeBindingProvenance::Direct,
                    )
                    .is_some()
                {
                    self.collect_associated_macro_calls(
                        module_id,
                        generated_source,
                        &trait_item.items,
                        &order,
                        &module_file_context,
                    );
                }
            }
            _ => {
                self.collect_named_def(
                    module_id,
                    &item,
                    generated_source,
                    item_id,
                    ScopeBindingProvenance::Direct,
                );
            }
        }

        Ok(())
    }

    /// Collects supported children of a generated extern block into the call-site module.
    fn collect_extern_block(
        &mut self,
        module_id: ModuleId,
        extern_block: &ExternBlockItem,
        generated_source: GeneratedSourceId,
        block_id: ItemTreeId,
    ) {
        let block_source = self.item_source(generated_source, block_id);
        for child_id in &extern_block.items {
            let child = self
                .state
                .def_map_builder
                .partial()
                .generated_source(generated_source)
                .and_then(|source| source.item(*child_id))
                .expect("generated extern-block child should exist while collecting def map")
                .clone();
            if !self.is_item_enabled(&child) {
                continue;
            }
            // Retained nested macro calls are intentionally not added to the expansion queue. The
            // ordinary local-def classifier rejects them here.
            if let Some(local_def) = self.collect_named_def(
                module_id,
                &child,
                generated_source,
                *child_id,
                ScopeBindingProvenance::Direct,
            ) {
                self.state
                    .def_map_builder
                    .insert_foreign_block(local_def, block_source);
            }
        }
    }

    fn is_item_enabled(&self, item: &ItemNode) -> bool {
        self.state.cfg_evaluator().is_enabled(&item.cfg)
    }

    fn collect_named_def(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        provenance: ScopeBindingProvenance,
    ) -> Option<LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let namespaces = kind.scope_namespaces(&item.kind);
        let name = item.name.clone()?;
        let visibility = item.visibility.clone();
        let visibilities = self.state.def_map_builder.resolve_local_def_visibilities(
            module_id,
            &item.kind,
            &visibility,
        );
        let local_def_id = self.state.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            namespaces,
            visibility: visibility.clone(),
            source: self.item_source(generated_source, item_id),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for generated local definition")
            .local_defs
            .push(local_def_id);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.state.crate_ref),
            local_def: local_def_id,
        });
        // Update both the base scopes and the current snapshot. The base scopes make future import
        // refreshes see the generated name; the current snapshot lets later generated calls in this
        // pass resolve it immediately.
        let base_scope = self
            .state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for generated local definition");
        let current_scope = self
            .current_scopes
            .module_scope_mut(self.state.crate_ref, module_id)
            .expect("current scope should exist for generated local definition");
        for namespace in namespaces.iter() {
            let binding = ScopeBinding::new(def, *visibilities.get(namespace), provenance);
            base_scope.insert_binding(&name, namespace, binding.clone());
            current_scope.insert_binding(&name, namespace, binding);
        }

        Some(local_def_id)
    }

    /// Record generated enum variants with the same namespace facts as source variants.
    fn collect_enum(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        enum_item: &rg_item_tree::EnumItem,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
    ) {
        let Some(local_def_id) = self.collect_named_def(
            module_id,
            item,
            generated_source,
            item_id,
            ScopeBindingProvenance::Direct,
        ) else {
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
        item: &ItemNode,
        macro_definition: &MacroDefinitionItem,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
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
        let Some(local_def_id) =
            self.collect_named_def(module_id, item, generated_source, item_id, provenance)
        else {
            return;
        };
        let Some(name) = item.name.clone() else {
            return;
        };

        self.state.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        if matches!(macro_definition, MacroDefinitionItem::MacroRules { .. }) {
            self.state.textual_macro_scopes.record_definition(
                module_id,
                name.clone(),
                local_def_id,
                order,
            );
        }
        if let MacroDefinitionItem::MacroRules { attrs, .. } = macro_definition
            && self.macro_definition_is_exported(attrs)
        {
            self.export_macro_definition_to_root(&name, local_def_id);
        }
        // Generated macro definitions inherit `$crate` from the macro that produced them, not from
        // the module where the generated definition is inserted.
        let dollar_crate = self.origin.dollar_crate.unwrap_or(self.state.crate_ref);
        self.state.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_item(
                macro_definition,
                item.docs.clone(),
                self.state.edition,
                dollar_crate,
            ),
        );
    }

    /// Updates both scope snapshots for a generated `#[macro_export]` definition.
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
            .expect("root scope should exist before generated macro export collection")
            .insert_binding(name, Namespace::Macros, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, root_module)
            .expect("current root scope should exist for generated macro export")
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

    #[allow(clippy::too_many_arguments)]
    fn collect_module(
        &mut self,
        parent_module: ModuleId,
        item: &ItemNode,
        module_item: &ModuleItem,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        order: ItemOrder,
        module_file_context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        // DefMap describes the requested source, while the project-owned coordinator resolves and
        // lowers it before resuming this retained construction state. Until a resolution exists, do
        // not fabricate an empty semantic module for a declaration with no file contents.
        let (origin, inner_docs, resolved_file) = match &module_item.source {
            ModuleSource::Inline { .. } => (
                ModuleOrigin::Inline {
                    declaration_file: item.file_id,
                    declaration_span: item.span,
                },
                module_item.inner_docs.clone(),
                None,
            ),
            ModuleSource::OutOfLine => {
                let request = MacroSourceFileRequest::module(
                    self.state.crate_ref.package,
                    Arc::clone(&module_file_context),
                    module_name.to_string(),
                    module_item.path_override.clone(),
                );
                match self
                    .macro_source_file_resolutions
                    .and_then(|resolutions| resolutions.get(&request).cloned())
                {
                    None => {
                        self.state.macro_source_file_requests.push(request.clone());
                        self.state
                            .pending_generated_modules
                            .push(PendingGeneratedModule {
                                request,
                                parent_module,
                                generated_source,
                                item_id,
                                order,
                                module_file_context,
                                origin: self.origin.clone(),
                            });
                        return Ok(());
                    }
                    Some(MacroSourceFileResolution::Missing) => return Ok(()),
                    Some(MacroSourceFileResolution::Module {
                        file_id: definition_file,
                        child_context,
                    }) => {
                        let item_tree_package = self
                            .item_tree
                            .package(self.state.crate_ref.package.0)
                            .context(
                                "while attempting to fetch package for generated module source",
                            )?;
                        let inner_docs = item_tree_package
                            .file(definition_file)
                            .with_context(|| {
                                format!(
                                    "while attempting to fetch generated module file {definition_file:?}"
                                )
                            })?
                            .docs
                            .clone();
                        (
                            ModuleOrigin::OutOfLine {
                                declaration_file: item.file_id,
                                declaration_span: item.span,
                                definition_file: Some(definition_file),
                                file_selection: ModuleFileSelection::from_path_override(
                                    module_item.path_override.as_deref(),
                                ),
                            },
                            inner_docs,
                            Some((definition_file, child_context)),
                        )
                    }
                    Some(MacroSourceFileResolution::Include { .. }) => {
                        unreachable!("generated module request received an include resolution")
                    }
                }
            }
        };

        let visibility = item.visibility.clone();
        let semantic_visibility = self
            .state
            .def_map_builder
            .resolve_visibility(parent_module, &visibility);
        let child_module = self.state.def_map_builder.alloc_module(ModuleData {
            name: Some(module_name.clone()),
            name_span: item.name_span,
            docs: Documentation::concat(item.docs.clone(), inner_docs),
            user_facing_attrs: item.user_facing_attrs,
            visibility: semantic_visibility,
            parent: Some(parent_module),
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin,
        });
        // Generated modules extend all scope matrices in lockstep with the def-map module arena so
        // later generated children or file-backed declarations can enter the new module.
        self.state.base_scopes.push(Default::default());
        self.state
            .textual_macro_scopes
            .record_module_declaration(child_module, order.clone());
        self.current_scopes
            .push_module_scope(self.state.crate_ref, Default::default())
            .expect("current scopes should have a crate slot for generated module");
        self.state
            .def_map_builder
            .module_mut(parent_module)
            .expect("parent module should exist for generated child link")
            .children
            .push((module_name.clone(), child_module));
        let binding = ScopeBinding::new(
            DefId::Module(ModuleRef {
                origin: DefMapRef::Crate(self.state.crate_ref),
                module: child_module,
            }),
            semantic_visibility,
            ScopeBindingProvenance::Direct,
        );
        self.state
            .base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, parent_module)
            .expect("current scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding);

        match &module_item.source {
            ModuleSource::Inline { items } => {
                let child_context = Arc::new(
                    module_file_context
                        .descend_inline(module_name.as_str(), module_item.path_override.as_deref()),
                );
                for (index, child_item) in items.iter().copied().enumerate() {
                    self.collect_item(
                        child_module,
                        generated_source,
                        child_item,
                        order.generated_child(index),
                        Arc::clone(&child_context),
                    )?;
                }
            }
            ModuleSource::OutOfLine => {
                let (definition_file, child_context) = resolved_file.expect(
                    "resolved out-of-line generated module should have a file and child context",
                );
                let item_tree_package = self
                    .item_tree
                    .package(self.state.crate_ref.package.0)
                    .context("while attempting to fetch generated module item tree package")?;
                let collected = SourceFragmentCollector {
                    state: self.state,
                    current_scopes: self.current_scopes,
                    item_tree: item_tree_package,
                    origin: SourceFragmentOrigin {
                        module: child_module,
                        order: ItemOrder::real(0),
                        parent_call: self.origin.parent_call,
                        placement: MacroCallPlacement::ModuleItems,
                        module_file_context: child_context,
                    },
                    result: MacroExpansionApplyResult::default(),
                    active_files: Default::default(),
                }
                .collect_module_file(definition_file)?;
                self.result.merge(collected);
            }
        }

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

    fn collect_macro_call(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        macro_call: &MacroCallItem,
        order: ItemOrder,
        placement: MacroCallPlacement,
        module_file_context: Arc<ModuleFileContext>,
    ) {
        // Macro-generated `include!("...")` remains a separate source-splicing feature. Carrying
        // module context is enough for `mod child;`, but it does not define include ownership or
        // source identity, so the generated lowerer still records no builtin payload here.
        self.state.push_macro_call(
            MacroCallSite {
                module: module_id,
                source: self.origin.source,
                path: macro_call.path.clone(),
                callee: macro_call.callee.clone(),
                args: macro_call.args.clone(),
                builtin: macro_call.builtin.clone(),
                dollar_crate: self.origin.dollar_crate,
                file_id: item.file_id,
                span: item.span,
                order,
                placement,
                module_file_context,
            },
            MacroCallOrigin::Generated {
                parent_call: self.origin.parent_call,
            },
        );
    }

    /// Queues macros retained inside a generated trait or impl declaration.
    fn collect_associated_macro_calls(
        &mut self,
        module_id: ModuleId,
        generated_source: GeneratedSourceId,
        item_ids: &[ItemTreeId],
        order: &ItemOrder,
        module_file_context: &Arc<ModuleFileContext>,
    ) {
        for (index, item_id) in item_ids.iter().copied().enumerate() {
            let item = self
                .state
                .def_map_builder
                .partial()
                .generated_source(generated_source)
                .and_then(|source| source.item(item_id))
                .expect("generated associated item should exist while collecting def map")
                .clone();
            if !self.is_item_enabled(&item) {
                continue;
            }
            let ItemKind::MacroCall(macro_call) = &item.kind else {
                continue;
            };
            let call_source = self.item_source(generated_source, item_id);
            self.collect_macro_call(
                module_id,
                &item,
                macro_call,
                order.generated_child(index),
                MacroCallPlacement::AssociatedItems { call_source },
                Arc::clone(module_file_context),
            );
        }
    }

    /// Retains one associated macro's output and queues nested calls in the same owner slot.
    ///
    /// If `impl User { methods!(); }` expands to `fn direct(&self) {}` plus `more_methods!();`,
    /// both sources replace `methods!`. The nested call is also queued as associated, so its later
    /// output replaces `more_methods!` inside the same impl rather than entering the module.
    fn collect_associated_expansion(
        &mut self,
        call_source: ItemSource,
        generated_source: GeneratedSourceId,
        item_ids: &[ItemTreeId],
    ) {
        let mut generated_items = Vec::new();
        for (index, item_id) in item_ids.iter().copied().enumerate() {
            let item = self
                .state
                .def_map_builder
                .partial()
                .generated_source(generated_source)
                .and_then(|source| source.item(item_id))
                .expect("generated associated expansion item should exist")
                .clone();
            if !self.is_item_enabled(&item) {
                continue;
            }

            let source = self.item_source(generated_source, item_id);
            match &item.kind {
                ItemKind::Const(_) | ItemKind::Function(_) | ItemKind::TypeAlias(_) => {
                    generated_items.push(source);
                }
                ItemKind::MacroCall(macro_call) => {
                    generated_items.push(source);
                    self.collect_macro_call(
                        self.origin.module,
                        &item,
                        macro_call,
                        self.origin.order.generated_child(index),
                        MacroCallPlacement::AssociatedItems {
                            call_source: source,
                        },
                        Arc::clone(&self.origin.module_file_context),
                    );
                }
                _ => {}
            }
        }
        self.state
            .def_map_builder
            .insert_associated_macro_expansion(call_source, generated_items);
    }

    fn collect_local_impl(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
    ) {
        let local_impl_id = self.state.def_map_builder.alloc_local_impl(LocalImplData {
            module: module_id,
            source: self.item_source(generated_source, item_id),
            file_id: item.file_id,
            span: item.span,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for generated impl block")
            .impls
            .push(local_impl_id);
    }

    fn collect_use(&mut self, module_id: ModuleId, item: &ItemNode, use_item: &UseItem) {
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let Some(mut path) = ImportPath::from_use_path(&import.path, self.origin.dollar_crate)
            else {
                continue;
            };
            if path.semantic().is_empty() {
                continue;
            }

            // The generated import's textual source is synthetic. Keep spans at the macro call site
            // so diagnostics and navigation have a real file location to point at.
            path.rebase(self.origin.span);
            let visibility = self
                .state
                .def_map_builder
                .resolve_visibility(module_id, &item.visibility);

            let import_id = self.state.def_map_builder.alloc_import(ImportData {
                module: module_id,
                visibility,
                is_reexport: ImportData::source_visibility_reexports(&item.visibility),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { .. } => Some(self.origin.span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: self.origin.source.into(),
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.state
                .def_map_builder
                .module_mut(module_id)
                .expect("module should exist for generated import")
                .imports
                .push(import_id);
        }
    }

    fn item_source(&self, generated_source: GeneratedSourceId, item: ItemTreeId) -> ItemSource {
        ItemSource::generated(
            self.origin.file_id,
            GeneratedItemRef {
                source: generated_source,
                item,
            },
        )
    }
}

/// Applies answered source requests to macro work that paused during collection.
///
/// The two continuation lists encode different Rust operations. A found `mod generated;` answer
/// allocates a child module, then collects the file inside it. A found `include!(...)` answer
/// splices the file according to the retained call placement and completes that macro directive.
/// Requests without answers remain pending and are re-emitted; an explicit missing answer finishes
/// the corresponding operation without inventing an empty source.
pub(crate) fn apply_pending_macro_source_files(
    item_tree: &ItemTreeDb,
    states: &mut super::super::finalize::FinalizeCrateStates,
    interners: &mut PackageNameInterners,
    current_scopes: &mut ScopeMatrix,
    macro_source_file_resolutions: Option<&MacroSourceFileResolutions>,
) -> Result<()> {
    for (package_slot, package_states) in states.iter_dirty_mut_enumerated() {
        let interner = interners.package_mut(package_slot).with_context(|| {
            format!("while attempting to fetch name interner for package {package_slot}")
        })?;

        for state in package_states {
            // Generated modules could not be allocated before their child file and file context
            // existed. Replay just that declaration through the normal generated-item collector.
            let pending_modules = std::mem::take(&mut state.pending_generated_modules);
            for pending in pending_modules {
                match macro_source_file_resolutions
                    .and_then(|resolutions| resolutions.get(&pending.request).cloned())
                {
                    None => {
                        state
                            .macro_source_file_requests
                            .push(pending.request.clone());
                        state.pending_generated_modules.push(pending);
                    }
                    Some(MacroSourceFileResolution::Missing) => {}
                    Some(MacroSourceFileResolution::Module { .. }) => {
                        let item = state
                            .def_map_builder
                            .partial()
                            .generated_source(pending.generated_source)
                            .and_then(|source| source.item(pending.item_id))
                            .expect("pending generated module item should remain available")
                            .clone();
                        let ItemKind::Module(module_item) = &item.kind else {
                            unreachable!("pending generated module should point to a module item");
                        };
                        GeneratedCollector {
                            state,
                            interner,
                            current_scopes,
                            item_tree,
                            macro_source_file_resolutions,
                            origin: pending.origin,
                            result: MacroExpansionApplyResult::default(),
                        }
                        .collect_module(
                            pending.parent_module,
                            &item,
                            module_item,
                            pending.generated_source,
                            pending.item_id,
                            pending.order,
                            pending.module_file_context,
                        )?;
                    }
                    Some(MacroSourceFileResolution::Include { .. }) => {
                        unreachable!("pending module received an include resolution")
                    }
                }
            }

            // Included items do not create a module. Collect the answered file at the retained
            // call-site order and module context, then settle the original macro directive.
            let pending_includes = std::mem::take(&mut state.pending_generated_includes);
            for pending in pending_includes {
                match macro_source_file_resolutions
                    .and_then(|resolutions| resolutions.get(&pending.request).cloned())
                {
                    None => {
                        state
                            .macro_source_file_requests
                            .push(pending.request.clone());
                        state.pending_generated_includes.push(pending);
                    }
                    Some(MacroSourceFileResolution::Missing) => {
                        if let Some(directive) = state.macro_directives.get_mut(pending.call_id) {
                            directive.state = super::MacroDirectiveState::Failed;
                        }
                    }
                    Some(MacroSourceFileResolution::Include { file_id }) => {
                        let item_tree_package =
                            item_tree.package(state.crate_ref.package.0).context(
                                "while attempting to fetch package for generated include source",
                            )?;
                        let collected = SourceFragmentCollector {
                            state,
                            current_scopes,
                            item_tree: item_tree_package,
                            origin: SourceFragmentOrigin {
                                module: pending.origin.module,
                                order: pending.origin.order,
                                parent_call: pending.call_id,
                                placement: pending.origin.placement,
                                module_file_context: pending.origin.module_file_context,
                            },
                            result: MacroExpansionApplyResult::default(),
                            active_files: Default::default(),
                        }
                        .collect_file(file_id);
                        let directive_state = if collected.is_ok() {
                            super::MacroDirectiveState::Expanded
                        } else {
                            super::MacroDirectiveState::Failed
                        };
                        if let Some(directive) = state.macro_directives.get_mut(pending.call_id) {
                            directive.state = directive_state;
                        }
                    }
                    Some(MacroSourceFileResolution::Module { .. }) => {
                        unreachable!("pending include received a module resolution")
                    }
                }
            }
        }
    }

    Ok(())
}
