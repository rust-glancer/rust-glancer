//! Collects declarations from written files, included fragments, and macro output.
//!
//! Every item carries its existing `ItemSource`, so the same Rust declaration handling can read
//! real ItemTrees or the transient generated arena. Only generated items need a per-item clone:
//! collection changes the builder that owns their payload. Written items stay borrowed.
//!
//! Real module contents start their own file order. Included fragments and expansion output instead
//! extend the invocation's order and retain its module or associated-item placement. Those inputs
//! are carried explicitly; a physical filename does not decide semantic ownership.

use std::{borrow::Cow, collections::HashSet, sync::Arc};

use anyhow::{Context as _, Result};
use rg_ir_model::{
    CrateRef, DefId, DefMapRef, FileId, LocalDefId, LocalDefRef, ModuleId, ModuleRef,
};
use rg_item_tree::{
    EnumItem, ExternBlockItem, ExternCrateItem, FunctionItem, ImportAlias, ItemKind, ItemNode,
    ItemTreeId, ItemTreeRef, MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem, ModuleItem,
    ModuleSource, Package as ItemTreePackage, UseImport, UseItem, VisibilityLevel,
};
use rg_parse::ModuleFileContext;
use rg_text::Name;
use rg_workspace::TargetKind;

use crate::{
    GeneratedItemRef, GeneratedSourceId, ImportBinding, ImportData, ImportKind, ImportPath,
    ItemSource, ItemSourceKind, LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData,
    MacroSourceFileRequest, ModuleBuilder, ModuleDocumentation, ModuleFileSelection, ModuleOrigin,
    Namespace, NamespaceSet, ScopeBinding, ScopeBindingProvenance, Visibility,
    build::{
        MacroSourceFileResolution, MacroSourceFileResolutions,
        crates::CrateBuilder,
        macros::{
            GeneratedOrigin, ItemOrder, MacroCallOrigin, MacroCallPlacement, MacroCallSite,
            MacroDefinitionRecord, MacroUseImport, PendingGeneratedModule,
        },
    },
    profile::metric,
};

/// Adds declaration facts to one crate, regardless of which item arena supplied the syntax.
///
/// `origin` keeps the diagnostic parent of newly queued calls. `dollar_crate` applies only to
/// generated syntax: entering a real child file or included fragment gives its definitions their
/// own crate again. Lookup and macro execution happen outside this collector. Collection reports
/// whether it added declaration facts or queued calls, so the caller knows when imports need replay.
pub(crate) struct DeclarationCollector<'a> {
    state: &'a mut CrateBuilder,
    item_tree: &'a ItemTreePackage,
    resolutions: &'a MacroSourceFileResolutions,
    origin: MacroCallOrigin,
    dollar_crate: Option<CrateRef>,
    changed: bool,
    active_files: HashSet<FileId>,
}

impl<'a> DeclarationCollector<'a> {
    pub(crate) fn new(
        state: &'a mut CrateBuilder,
        item_tree: &'a ItemTreePackage,
        resolutions: &'a MacroSourceFileResolutions,
        origin: MacroCallOrigin,
        dollar_crate: Option<CrateRef>,
    ) -> Self {
        Self {
            state,
            item_tree,
            resolutions,
            origin,
            dollar_crate,
            changed: false,
            active_files: HashSet::new(),
        }
    }

    /// Enter a target root or a real child module, with ordinary file order.
    pub(crate) fn collect_module_file(
        mut self,
        module: ModuleId,
        file: FileId,
        context: Arc<ModuleFileContext>,
    ) -> Result<bool> {
        self.collect_file_items(
            module,
            file,
            ItemOrder::real,
            MacroCallPlacement::ModuleItems,
            context,
        )?;
        Ok(self.changed)
    }

    /// Splice a real include at its invocation, keeping its source refs and placement.
    pub(crate) fn collect_file(
        mut self,
        module: ModuleId,
        file: FileId,
        order: ItemOrder,
        placement: MacroCallPlacement,
        context: Arc<ModuleFileContext>,
    ) -> Result<bool> {
        self.collect_file_items(
            module,
            file,
            |index| order.generated_child(index),
            placement,
            context,
        )?;
        Ok(self.changed)
    }

    /// Splice a builtin-selected subset already lowered in a real file.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect_fragment(
        mut self,
        module: ModuleId,
        file: FileId,
        items: &[ItemTreeId],
        order: ItemOrder,
        placement: MacroCallPlacement,
        context: Arc<ModuleFileContext>,
    ) -> Result<bool> {
        // This subset still belongs to the active file. A child module pointing back to that
        // file must not re-enter its full contents while this fragment is being collected.
        self.active_files.insert(file);
        self.collect_items(
            module,
            items.iter().map(|item| {
                ItemTreeRef {
                    file_id: file,
                    item: *item,
                }
                .into()
            }),
            |index| order.generated_child(index),
            placement,
            context,
        )?;
        Ok(self.changed)
    }

    pub(crate) fn collect_expansion(
        mut self,
        origin: GeneratedOrigin,
        generated_source: GeneratedSourceId,
    ) -> Result<bool> {
        self.changed = true;
        let items = self
            .state
            .def_map_builder
            .generated_source(generated_source)
            .expect("new generated source should exist")
            .top_level
            .clone();
        self.collect_items(
            origin.module,
            items.into_iter().map(|item| {
                ItemSource::generated(
                    origin.file_id,
                    GeneratedItemRef {
                        source: generated_source,
                        item,
                    },
                )
            }),
            |index| origin.order.generated_child(index),
            origin.placement,
            origin.module_file_context,
        )?;
        Ok(self.changed)
    }

    /// Complete only the deferred module declaration; its surrounding expansion was already collected.
    pub(crate) fn collect_pending_module(mut self, pending: PendingGeneratedModule) -> Result<()> {
        let item = self.item(pending.source);
        let ItemKind::Module(module_item) = &item.kind else {
            unreachable!("pending module should point to a module item");
        };
        self.collect_module(
            pending.parent_module,
            &item,
            pending.source,
            module_item,
            pending.order,
            pending.module_file_context,
        )
    }

    /// Borrow real items independently of the builder borrow; clone generated items one at a time.
    fn item(&self, source: ItemSource) -> Cow<'a, ItemNode> {
        match source.kind {
            ItemSourceKind::ItemTree(source) => Cow::Borrowed(
                self.item_tree
                    .item(source)
                    .expect("written item should exist while collecting declarations"),
            ),
            ItemSourceKind::Generated(source) => Cow::Owned(
                self.state
                    .def_map_builder
                    .generated_source(source.source)
                    .and_then(|data| data.item(source.item))
                    .expect("generated item should exist while collecting declarations")
                    .clone(),
            ),
            ItemSourceKind::Synthetic(_) | ItemSourceKind::Body(_) => {
                unreachable!("crate collection needs written or generated items")
            }
        }
    }

    /// Stop cycles on the active source path, while allowing another interpretation after returning.
    fn collect_file_items(
        &mut self,
        module: ModuleId,
        file: FileId,
        order_for: impl Fn(usize) -> ItemOrder,
        placement: MacroCallPlacement,
        context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        if !self.active_files.insert(file) {
            return Ok(());
        }
        let tree = self
            .item_tree
            .file(file)
            .context("module item tree should exist")?;
        let result = self.collect_items(
            module,
            tree.top_level.iter().map(|item| {
                ItemTreeRef {
                    file_id: file,
                    item: *item,
                }
                .into()
            }),
            order_for,
            placement,
            context,
        );
        self.active_files.remove(&file);
        result
    }

    /// Apply the same cfg and declaration rules to every source; associated output stays in its owner.
    fn collect_items(
        &mut self,
        module: ModuleId,
        items: impl IntoIterator<Item = ItemSource>,
        order_for: impl Fn(usize) -> ItemOrder,
        placement: MacroCallPlacement,
        context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        let mut associated_items = Vec::new();
        for (index, source) in items.into_iter().enumerate() {
            let item = self.item(source);
            if !self.is_item_enabled(&item) {
                continue;
            }
            self.changed = true;
            let order = order_for(index);
            match placement {
                MacroCallPlacement::ModuleItems => {
                    if matches!(source.kind, ItemSourceKind::Generated(_)) {
                        metric::GENERATED_ITEMS_SEEN.inc();
                    }
                    self.collect_item(module, &item, source, order, Arc::clone(&context))?;
                }
                MacroCallPlacement::AssociatedItems { .. } => match &item.kind {
                    ItemKind::Const(_) | ItemKind::Function(_) | ItemKind::TypeAlias(_) => {
                        associated_items.push(source)
                    }
                    ItemKind::MacroCall(call) => {
                        associated_items.push(source);
                        self.collect_macro_call(
                            module,
                            &item,
                            source,
                            call,
                            order,
                            MacroCallPlacement::AssociatedItems {
                                call_source: source,
                            },
                            Arc::clone(&context),
                        );
                    }
                    // Other declarations do not produce supported associated-item facts.
                    _ => {}
                },
            }
        }
        if let MacroCallPlacement::AssociatedItems { call_source } = placement {
            self.state
                .def_map_builder
                .insert_associated_macro_expansion(call_source, associated_items);
        }
        Ok(())
    }

    fn collect_item(
        &mut self,
        module: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        order: ItemOrder,
        context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        match &item.kind {
            ItemKind::ExternBlock(block) => self.collect_extern_block(module, source, block),
            ItemKind::ExternCrate(extern_crate) => {
                self.collect_extern_crate(module, item, extern_crate)
            }
            ItemKind::Module(child) => {
                self.collect_module(module, item, source, child, order, context)?
            }
            ItemKind::Use(use_item) => self.collect_use(module, item, source, use_item),
            ItemKind::Enum(enum_item) => self.collect_enum(module, item, source, enum_item),
            ItemKind::Impl(impl_item) => {
                self.collect_local_impl(module, item, source);
                self.collect_associated_macro_calls(
                    module,
                    source,
                    &impl_item.items,
                    &order,
                    &context,
                );
            }
            ItemKind::MacroCall(call) => self.collect_macro_call(
                module,
                item,
                source,
                call,
                order,
                MacroCallPlacement::ModuleItems,
                context,
            ),
            ItemKind::MacroDefinition(definition) => {
                self.collect_macro_definition(module, item, source, definition, order)
            }
            ItemKind::Function(function) => {
                if let Some(implementation) =
                    self.collect_local_def(module, item, source, ScopeBindingProvenance::Direct)
                {
                    self.collect_proc_macro_definition(
                        module,
                        item,
                        source,
                        function,
                        implementation,
                        order,
                    );
                }
            }
            ItemKind::Trait(trait_item) => {
                if self
                    .collect_local_def(module, item, source, ScopeBindingProvenance::Direct)
                    .is_some()
                {
                    self.collect_associated_macro_calls(
                        module,
                        source,
                        &trait_item.items,
                        &order,
                        &context,
                    );
                }
            }
            _ => {
                self.collect_local_def(module, item, source, ScopeBindingProvenance::Direct);
            }
        }
        Ok(())
    }

    /// Queue a call with its real invocation source and its diagnostic expansion parent.
    #[allow(clippy::too_many_arguments)]
    fn collect_macro_call(
        &mut self,
        module: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        call: &MacroCallItem,
        order: ItemOrder,
        placement: MacroCallPlacement,
        context: Arc<ModuleFileContext>,
    ) {
        let (source, dollar_crate) = match source.kind {
            ItemSourceKind::Generated(generated) => (
                self.state
                    .def_map_builder
                    .generated_source(generated.source)
                    .expect("generated call source should exist")
                    .origin_source,
                self.dollar_crate,
            ),
            ItemSourceKind::ItemTree(written) => (written, None),
            _ => unreachable!("crate macro calls need written or generated source"),
        };
        self.state.push_macro_call(
            MacroCallSite {
                module,
                source,
                path: call.path.clone(),
                callee: call.callee.clone(),
                args: call.args.clone(),
                builtin: call.builtin.clone(),
                dollar_crate,
                file_id: item.file_id,
                span: item.span,
                order,
                placement,
                module_file_context: context,
            },
            self.origin,
        );
    }

    /// A module declaration owns a child scope; `include!` uses the caller's scope instead.
    #[allow(clippy::too_many_arguments)]
    fn collect_module(
        &mut self,
        parent_module: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        module_item: &ModuleItem,
        order: ItemOrder,
        context: Arc<ModuleFileContext>,
    ) -> Result<()> {
        let Some(name) = item.name.clone() else {
            return Ok(());
        };
        let generated = matches!(source.kind, ItemSourceKind::Generated(_));
        let resolved_file = match &module_item.source {
            ModuleSource::Inline { .. } => None,
            ModuleSource::OutOfLine if generated => {
                let request = MacroSourceFileRequest::module(
                    self.state.crate_ref.package,
                    Arc::clone(&context),
                    name.to_string(),
                    module_item.path_override.clone(),
                );
                match self.resolutions.get(&request) {
                    None => {
                        self.state.macro_source_file_requests.push(request.clone());
                        self.state
                            .pending_generated_modules
                            .push(PendingGeneratedModule {
                                request,
                                parent_module,
                                source,
                                order,
                                module_file_context: context,
                                origin: self.origin,
                            });
                        return Ok(());
                    }
                    // TODO: Choose a common missing-module policy; written source retains the declaration.
                    Some(MacroSourceFileResolution::Missing) => return Ok(()),
                    Some(MacroSourceFileResolution::Module {
                        file_id,
                        child_context,
                    }) => Some((*file_id, Arc::clone(child_context))),
                    Some(MacroSourceFileResolution::Include { .. }) => {
                        unreachable!("module request received an include answer")
                    }
                }
            }
            ModuleSource::OutOfLine => self.state.resolve_module_file(
                &context,
                name.as_str(),
                module_item.path_override.as_deref(),
            ),
        };
        let (origin, inner_docs) = match &module_item.source {
            ModuleSource::Inline { .. } => (
                ModuleOrigin::Inline {
                    declaration_file: item.file_id,
                    declaration_span: item.span,
                },
                module_item.inner_docs.clone(),
            ),
            ModuleSource::OutOfLine => (
                ModuleOrigin::OutOfLine {
                    declaration_file: item.file_id,
                    declaration_span: item.span,
                    definition_file: resolved_file.as_ref().map(|(file, _)| *file),
                    file_selection: ModuleFileSelection::from_path_override(
                        module_item.path_override.as_deref(),
                    ),
                },
                match &resolved_file {
                    Some((file, _)) => self
                        .item_tree
                        .file(*file)
                        .context("module docs should exist")?
                        .docs
                        .clone(),
                    None => None,
                },
            ),
        };
        let visibility = self
            .state
            .def_map_builder
            .resolve_visibility(parent_module, &item.visibility);
        // Allocate the child's structure and scopes together, then expose it in the parent's
        // module tree and type namespace before collecting its contents.
        let mut module = ModuleBuilder::new(origin, visibility, Some(parent_module));
        module.name = Some(name.clone());
        module.name_span = item.name_span;
        module.docs = ModuleDocumentation::new(item.docs.clone(), inner_docs);
        module.user_facing_attrs = item.user_facing_attrs;
        let child = self.state.def_map_builder.alloc_module(module);
        self.state
            .def_map_builder
            .module_mut(parent_module)
            .expect("parent module should exist for child link")
            .children
            .push((name.clone(), child));
        self.state.def_map_builder.insert_direct_binding(
            parent_module,
            &name,
            Namespace::Types,
            ScopeBinding::new(
                DefId::Module(ModuleRef::krate(self.state.crate_ref, child)),
                visibility,
                ScopeBindingProvenance::Direct,
            ),
        );
        self.state
            .textual_macro_scopes
            .record_module_declaration(child, order.clone());
        match &module_item.source {
            ModuleSource::Inline { items } => {
                let child_context = Arc::new(
                    context.descend_inline(name.as_str(), module_item.path_override.as_deref()),
                );
                // Real inline modules start their own order. Expanded inline descendants stay under
                // the invocation's order, preserving textual macro visibility across the expansion.
                self.collect_items(
                    child,
                    items.iter().map(|item| source.with_item(*item)),
                    |index| {
                        if generated {
                            order.generated_child(index)
                        } else {
                            ItemOrder::real(index)
                        }
                    },
                    MacroCallPlacement::ModuleItems,
                    child_context,
                )?;
            }
            ModuleSource::OutOfLine => {
                let Some((file, child_context)) = resolved_file else {
                    return Ok(());
                };
                self.collect_file_items(
                    child,
                    file,
                    ItemOrder::real,
                    MacroCallPlacement::ModuleItems,
                    child_context,
                )?;
            }
        }
        if let Some(macro_use) = &module_item.macro_use
            && let Some(selector) = macro_use.active_selector(&self.state.cfg_evaluator())
        {
            self.state.textual_macro_scopes.import_module_definitions(
                parent_module,
                child,
                order,
                &selector,
            );
        }
        Ok(())
    }

    /// Expose supported foreign declarations in the enclosing module and retain their extern-block owner.
    fn collect_extern_block(
        &mut self,
        module_id: ModuleId,
        block_source: ItemSource,
        extern_block: &ExternBlockItem,
    ) {
        for child_id in &extern_block.items {
            let child_source = block_source.with_item(*child_id);
            let child = self.item(child_source);
            if !self.is_item_enabled(&child) {
                continue;
            }
            // TODO: Support macros in extern blocks. Until then, collect only named declarations;
            // nested calls stay in ItemTree without entering the expansion queue.
            if let Some(local_def) = self.collect_local_def(
                module_id,
                &child,
                child_source,
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

    /// Allocate a named declaration and insert its bindings into each namespace it occupies.
    fn collect_local_def(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemSource,
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

        // Keep the item's source identity for Semantic IR while making its name visible now.
        let local_def_id = self.state.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            namespaces,
            visibility: item.visibility.clone(),
            source,
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for local definition")
            .local_defs
            .push(local_def_id);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.state.crate_ref),
            local_def: local_def_id,
        });
        for namespace in namespaces.iter() {
            self.state.def_map_builder.insert_direct_binding(
                module_id,
                &name,
                namespace,
                ScopeBinding::new(def, *visibilities.get(namespace), provenance),
            );
        }

        Some(local_def_id)
    }

    /// Retain variant targets for qualified paths and imports without adding bare names to the module.
    fn collect_enum(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        enum_item: &EnumItem,
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

    /// Keep the macro body and textual order alongside its ordinary namespace binding.
    fn collect_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemSource,
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
        // Expanded macro definitions inherit the producing macro's `$crate`. Real files keep
        // their own crate even when they were loaded because of a generated declaration.
        self.state.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_item(
                macro_definition,
                item.docs.clone(),
                self.state.edition,
                if matches!(source.kind, ItemSourceKind::Generated(_)) {
                    self.dollar_crate.unwrap_or(self.state.crate_ref)
                } else {
                    self.state.crate_ref
                },
            ),
        );
    }

    /// Make a `#[macro_export]` definition visible through the crate root.
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

        self.state.def_map_builder.insert_direct_binding(
            root_module,
            name,
            Namespace::Macros,
            binding,
        );
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

    /// Queue macros inside a trait or impl, keeping their output in that associated-item slot.
    fn collect_associated_macro_calls(
        &mut self,
        module_id: ModuleId,
        parent_source: ItemSource,
        item_ids: &[ItemTreeId],
        order: &ItemOrder,
        module_file_context: &Arc<ModuleFileContext>,
    ) {
        for (index, item_id) in item_ids.iter().copied().enumerate() {
            let source = parent_source.with_item(item_id);
            let item = self.item(source);
            if !self.is_item_enabled(&item) {
                continue;
            }
            let ItemKind::MacroCall(macro_call) = &item.kind else {
                continue;
            };
            self.collect_macro_call(
                module_id,
                &item,
                source,
                macro_call,
                order.generated_child(index),
                MacroCallPlacement::AssociatedItems {
                    call_source: source,
                },
                Arc::clone(module_file_context),
            );
        }
    }

    /// Record an impl for Semantic IR without introducing a namespace binding.
    fn collect_local_impl(&mut self, module_id: ModuleId, item: &ItemNode, source: ItemSource) {
        let local_impl_id = self.state.def_map_builder.alloc_local_impl(LocalImplData {
            module: module_id,
            source,
            file_id: item.file_id,
            span: item.span,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for impl block")
            .impls
            .push(local_impl_id);
    }

    /// Record import paths and aliases for later resolution, preserving their source provenance.
    fn collect_use(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        use_item: &UseItem,
    ) {
        // Written imports retain their own locations. Generated paths and aliases point at the
        // real invocation, while their semantic paths still carry the expansion's `$crate`.
        let (source, call_span, dollar_crate) = match source.kind {
            ItemSourceKind::Generated(generated) => {
                let data = self
                    .state
                    .def_map_builder
                    .generated_source(generated.source)
                    .expect("generated import source should exist");
                (
                    data.origin_source.into(),
                    Some(data.origin_span),
                    self.dollar_crate,
                )
            }
            _ => (source, None, None),
        };
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let Some(mut path) = ImportPath::from_use_path(&import.path, dollar_crate) else {
                continue;
            };
            if path.semantic().is_empty() {
                continue;
            }

            if let Some(span) = call_span {
                path.rebase(span);
            }
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
                    ImportAlias::Explicit { span, .. } => Some(call_span.unwrap_or(*span)),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source,
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.state
                .def_map_builder
                .module_mut(module_id)
                .expect("module should exist for import")
                .imports
                .push(import_id);
        }
    }

    /// Bind a known crate root immediately, including aliases and legacy macro-use imports.
    fn collect_extern_crate(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
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

        // Root declarations also enter the crate-wide extern prelude. A child declaration keeps
        // only its module binding, regardless of whether it was written, included, or generated.
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
        self.state.def_map_builder.insert_direct_binding(
            module_id,
            &binding_name,
            Namespace::Types,
            binding,
        );
    }

    /// Register an annotated root function as a separate exported macro identity.
    ///
    /// `#[proc_macro_derive(Stored)] fn stored(...)` keeps `stored` as a value definition and adds
    /// the macro name `Stored`, linked back to that implementation function.
    fn collect_proc_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemSource,
        function: &FunctionItem,
        implementation: LocalDefId,
        order: ItemOrder,
    ) {
        let Some(proc_macro) = &function.proc_macro else {
            return;
        };
        if !matches!(self.state.target_kind, TargetKind::ProcMacro)
            || module_id != self.state.root_module
        {
            return;
        }

        let local_def_id = self.state.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: proc_macro.name.clone(),
            kind: LocalDefKind::MacroDefinition,
            namespaces: NamespaceSet::MACROS,
            visibility: VisibilityLevel::Public,
            source,
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("proc-macro root module should exist")
            .local_defs
            .push(local_def_id);

        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.state.crate_ref),
            local_def: local_def_id,
        });
        self.state.def_map_builder.insert_direct_binding(
            module_id,
            &proc_macro.name,
            Namespace::Macros,
            ScopeBinding::new(def, Visibility::Public, ScopeBindingProvenance::Direct),
        );
        self.state.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        self.state.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_proc_macro(
                proc_macro.kind,
                implementation,
                item.docs.clone(),
                self.state.edition,
                self.state.crate_ref,
            ),
        );
    }
}
