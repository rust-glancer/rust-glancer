//! Collects and finalizes lexical declaration scopes as DefMap facts.
//!
//! Declaration scopes become synthetic modules. Direct declarations are collected first, then imports are
//! resolved in a fixed-point loop before the final DefMap is frozen.

use rg_def_map::{
    BodyItemSourceRef, CrateResolutionEnv, DefMap, DefMapBuilder, DefMapSource, ImportBinding,
    ImportData, ImportKind, ImportPath, ItemSource, LocalDefData, LocalDefKind,
    LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData, MacroDefinitionEnv,
    MacroDefinitionView, ModuleBuilder, ModuleDocumentation, ModuleFileSelection, ModuleOrigin,
    ModuleResolutionView, Namespace, ScopeBinding, ScopeBindingProvenance, ScopeEntryRef,
    ScopeResolutionEnv, ScopeResolver, Visibility,
};
use rg_ir_model::{
    BodyRef, CrateRef, DefId, DefMapRef, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleId,
    ModuleRef,
};
use rg_item_tree::{
    EnumItem, ExternBlockItem, ImportAlias, ItemKind, ItemNode, ItemTreeId, ModuleSource,
};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use super::LocalItemSource;

pub(crate) struct LocalDefMapCollector<'source> {
    body_ref: BodyRef,
    source: LocalItemSource<'source>,
    builder: DefMapBuilder,
    /// There might be more modules than scopes, so we need a mapping.
    /// Keys here are scope IDs, values are corresponding modules.
    modules_by_scope: Vec<ModuleId>,
}

impl<'source> LocalDefMapCollector<'source> {
    pub fn new(body_ref: BodyRef, source: LocalItemSource<'source>) -> Self {
        Self {
            body_ref,
            source,
            builder: DefMapBuilder::new(DefMapRef::Body(body_ref)),
            modules_by_scope: Vec::with_capacity(source.scopes.len()),
        }
    }

    /// Collects direct local scope facts. Imports are finalized in a separate fixed-point step.
    pub fn collect(
        mut self,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<LocalDefMapBuildState> {
        // First, go through all the scopes and allocate synthetic modules.
        for scope in self.source.scopes {
            rg_std::check_cancel!(cancellation, "local declaration scopes");
            // Body scopes are synthetic modules. They carry lexical scope data, but they do not
            // correspond to Rust module declarations and lookup must treat them differently.
            let origin = ModuleOrigin::Synthetic {
                file_id: self.source.source.file_id,
                span: self.source.source.span,
            };

            // Note: we're going through scopes in order, so we process all the parents first.
            let parent = scope.parent.map(|parent| self.modules_by_scope[parent.0]);
            // Synthetic scopes are reachable only through their body origin. Public visibility
            // adds no narrower module ceiling inside that body; it cannot export the scope.
            let module =
                self.builder
                    .alloc_module(ModuleBuilder::new(origin, Visibility::Public, parent));
            self.modules_by_scope.push(module);
        }

        // Second, go through all the items in each scope and collect them too.
        // Note that we are collecting _items_ from scopes, but here we do not
        // recurse: even if an item has a body, we do not start to analyze it.
        for (scope_id, scope) in self.source.scopes.iter().enumerate() {
            rg_std::check_cancel!(cancellation, "local declaration scopes");
            let module = *self
                .modules_by_scope
                .get(scope_id)
                .expect("Must be provided");
            for item in &scope.source_items {
                rg_std::check_cancel!(cancellation, "local declaration scopes");
                self.collect_item(module, *item);
            }
        }

        Ok(LocalDefMapBuildState {
            body_ref: self.body_ref,
            builder: self.builder,
        })
    }

    fn collect_item(&mut self, module: ModuleId, item_id: ItemTreeId) {
        let Some(item) = self.source.items.item(item_id) else {
            return;
        };

        match &item.kind {
            ItemKind::Module(module_item) => self.collect_module(module, item, module_item),
            ItemKind::Use(use_item) => self.collect_use(module, item_id, item, use_item),
            ItemKind::Impl(_) => self.collect_local_impl(module, item_id, item),
            ItemKind::Enum(enum_item) => self.collect_enum(module, item_id, item, enum_item),
            ItemKind::ExternBlock(extern_block) => {
                self.collect_extern_block(module, item_id, item, extern_block)
            }
            ItemKind::MacroCall(_)
            | ItemKind::MacroDefinition(_)
            | ItemKind::ExternCrate(_)
            | ItemKind::AsmExpr => {}
            _ => {
                self.collect_local_def(module, item_id, item);
            }
        }
    }

    fn collect_extern_block(
        &mut self,
        module: ModuleId,
        block_id: ItemTreeId,
        block: &ItemNode,
        extern_block: &ExternBlockItem,
    ) {
        let block_source = self.item_source(block_id, block);
        for child_id in &extern_block.items {
            let Some(child) = self.source.items.item(*child_id).cloned() else {
                continue;
            };
            // Body-local macro calls remain retained source items, but the ordinary local-def
            // classifier rejects them and this path never queues expansion.
            if let Some(local_def) = self.collect_local_def(module, *child_id, &child) {
                self.builder.insert_foreign_block(local_def, block_source);
            }
        }
    }

    fn collect_local_def(
        &mut self,
        module: ModuleId,
        item_id: ItemTreeId,
        item: &ItemNode,
    ) -> Option<rg_ir_model::LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let name = item.name.clone()?;
        let namespaces = kind.scope_namespaces(&item.kind);
        let visibilities =
            self.builder
                .resolve_local_def_visibilities(module, &item.kind, &item.visibility);

        let local_def = self.builder.alloc_local_def(LocalDefData {
            module,
            name: name.clone(),
            kind,
            namespaces,
            visibility: item.visibility.clone(),
            source: self.item_source(item_id, item),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.builder
            .module_mut(module)
            .expect("module should exist for body local definition")
            .local_defs
            .push(local_def);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Body(self.body_ref),
            local_def,
        });
        for namespace in namespaces.iter() {
            self.builder.insert_direct_binding(
                module,
                &name,
                namespace,
                ScopeBinding::new(
                    def,
                    *visibilities.get(namespace),
                    ScopeBindingProvenance::Direct,
                ),
            );
        }
        Some(local_def)
    }

    fn collect_enum(
        &mut self,
        module: ModuleId,
        item_id: ItemTreeId,
        item: &ItemNode,
        enum_item: &EnumItem,
    ) {
        let Some(local_def) = self.collect_local_def(module, item_id, item) else {
            return;
        };
        let visibility = self.builder.resolve_visibility(module, &item.visibility);

        self.builder.alloc_local_enum_variants(
            module,
            local_def,
            enum_item,
            visibility,
            item.file_id,
        );
    }

    fn collect_local_impl(&mut self, module: ModuleId, item_id: ItemTreeId, item: &ItemNode) {
        let local_impl = self.builder.alloc_local_impl(LocalImplData {
            module,
            source: self.item_source(item_id, item),
            file_id: item.file_id,
            span: item.span,
        });
        self.builder
            .module_mut(module)
            .expect("module should exist for body local impl")
            .impls
            .push(local_impl);
    }

    fn collect_module(
        &mut self,
        parent: ModuleId,
        item: &ItemNode,
        module_item: &rg_item_tree::ModuleItem,
    ) {
        let Some(module_name) = item.name.clone() else {
            return;
        };

        let origin = match &module_item.source {
            ModuleSource::Inline { .. } => ModuleOrigin::Inline {
                declaration_file: item.file_id,
                declaration_span: item.span,
            },
            ModuleSource::OutOfLine => ModuleOrigin::OutOfLine {
                declaration_file: item.file_id,
                declaration_span: item.span,
                definition_file: None,
                file_selection: ModuleFileSelection::from_path_override(
                    module_item.path_override.as_deref(),
                ),
            },
        };
        let docs = match &module_item.source {
            ModuleSource::Inline { .. } => {
                ModuleDocumentation::new(item.docs.clone(), module_item.inner_docs.clone())
            }
            ModuleSource::OutOfLine => ModuleDocumentation::new(item.docs.clone(), None),
        };
        let visibility = self.builder.resolve_visibility(parent, &item.visibility);

        let mut module = ModuleBuilder::new(origin, visibility, Some(parent));
        module.name = Some(module_name.clone());
        module.name_span = item.name_span;
        module.docs = docs;
        module.user_facing_attrs = item.user_facing_attrs;
        let child = self.builder.alloc_module(module);
        self.builder
            .module_mut(parent)
            .expect("parent module should exist for body child module")
            .children
            .push((module_name.clone(), child));
        self.builder.insert_direct_binding(
            parent,
            &module_name,
            Namespace::Types,
            ScopeBinding::new(
                DefId::Module(ModuleRef {
                    origin: DefMapRef::Body(self.body_ref),
                    module: child,
                }),
                visibility,
                ScopeBindingProvenance::Direct,
            ),
        );

        // Note: out-of-line modules are not parsed, it's a bizarre pattern and at
        // least for now it's not worth spending time on it.
        if let ModuleSource::Inline { items } = &module_item.source {
            for item in items {
                self.collect_item(child, *item);
            }
        }
    }

    fn collect_use(
        &mut self,
        module: ModuleId,
        item_id: ItemTreeId,
        item: &ItemNode,
        use_item: &rg_item_tree::UseItem,
    ) {
        for (import_index, import) in use_item.imports.iter().enumerate() {
            let Some(path) = ImportPath::from_use_path(&import.path, None) else {
                continue;
            };
            if path.semantic().is_empty() {
                continue;
            }

            let visibility = self.builder.resolve_visibility(module, &item.visibility);
            let import = self.builder.alloc_import(ImportData {
                module,
                visibility,
                is_reexport: ImportData::source_visibility_reexports(&item.visibility),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { span, .. } => Some(*span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: self.item_source(item_id, item),
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.builder
                .module_mut(module)
                .expect("module should exist for body import")
                .imports
                .push(import);
        }
    }

    fn item_source(&self, item_id: ItemTreeId, item: &ItemNode) -> ItemSource {
        ItemSource::body(
            item.file_id,
            BodyItemSourceRef {
                body: self.body_ref,
                item: item_id,
            },
        )
    }
}

/// Body-local DefMap state before imports have been fixed up and frozen.
pub(crate) struct LocalDefMapBuildState {
    body_ref: BodyRef,
    builder: DefMapBuilder,
}

impl LocalDefMapBuildState {
    /// Replay local imports until no body scope changes, then freeze the scopes and their reports.
    ///
    /// Other crates and bodies are already frozen. This loop reruns the active body's imports
    /// together, without the cross-package scheduling used during crate construction.
    pub(crate) fn finalize<S>(
        mut self,
        def_maps: S,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<DefMap>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        loop {
            rg_std::check_cancel!(cancellation, "local import round");
            let candidates = {
                // Rebuild from declarations so an import's old target can disappear on replay.
                let mut candidates = (0..self.builder.module_count())
                    .map(|index| {
                        (
                            self.builder
                                .module_direct_scope(ModuleId(index))
                                .expect("body module should have direct bindings")
                                .clone(),
                            Vec::new(),
                        )
                    })
                    .collect::<Vec<_>>();
                // Lookup reads the published scopes, not the candidates being filled. In
                // `use crate::Id as Alias; use Alias as Other;`, Other gains its target next wave.
                let env = LocalDefMapFinalizationEnv {
                    def_maps,
                    state: &self,
                };
                let resolver = ScopeResolver::new(&env);
                for (import_id, import) in self.builder.imports_with_ids() {
                    rg_std::check_cancel!(cancellation, "local import resolution");
                    let importing_module = ModuleRef {
                        origin: DefMapRef::Body(self.body_ref),
                        module: import.module,
                    };
                    let (scope, unresolved_imports) = &mut candidates[import.module.0];
                    let resolution = resolver.apply_import(
                        importing_module,
                        ImportRef {
                            origin: importing_module.origin,
                            import: import_id,
                        },
                        import,
                        scope,
                    )?;
                    if !resolution.is_resolved() {
                        unresolved_imports.push(import_id);
                    }
                }
                candidates
            };

            // All lookups are done. Publish each scope with the report from that same wave,
            // including the final unchanged wave whose reports will be frozen below.
            let mut stable = true;
            for (index, (scope, unresolved_imports)) in candidates.into_iter().enumerate() {
                let module = ModuleId(index);
                stable &= self.builder.module_scope(module) == Some(&scope);
                self.builder
                    .apply_module_imports(module, Some(scope), unresolved_imports);
            }
            if stable {
                return Ok(self.builder.build());
            }
        }
    }
}

/// Read builder scopes for the active body and frozen DefMaps for every other origin.
///
/// A local import can name crate items or aliases produced by earlier waves in this body. Metadata
/// is shared through a scope-free view; lookup reads the active body's resolved bindings separately.
struct LocalDefMapFinalizationEnv<'state, S> {
    def_maps: S,
    state: &'state LocalDefMapBuildState,
}

impl<S> LocalDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    fn is_active_body_origin(&self, origin: DefMapRef) -> bool {
        origin == DefMapRef::Body(self.state.body_ref)
    }
}

impl<S> ScopeResolutionEnv for LocalDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    type Error = PackageStoreError;

    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, Self::Error> {
        if self.is_active_body_origin(module_ref.origin) {
            return Ok(self
                .state
                .builder
                .module(module_ref.module)
                .map(ModuleBuilder::resolution_view));
        }

        Ok(self
            .def_maps
            .module_data(module_ref)?
            .map(|module| module.resolution_view()))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error> {
        if self.is_active_body_origin(module_ref.origin) {
            return Ok(self
                .state
                .builder
                .module_scope(module_ref.module)
                .and_then(|scope| scope.entry(name)));
        }

        Ok(self
            .def_maps
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, Self::Error> {
        if self.is_active_body_origin(module_ref.origin) {
            return Ok(self
                .state
                .builder
                .module_scope(module_ref.module)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }

        Ok(self
            .def_maps
            .module_data(module_ref)?
            .map(|module| {
                module
                    .scope
                    .entries()
                    .map(|(name, entry)| (name, entry.as_ref()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, Self::Error> {
        if self.is_active_body_origin(local_def_ref.origin) {
            return Ok(self.state.builder.local_def(local_def_ref.local_def));
        }

        self.def_maps.local_def_data(local_def_ref)
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        if self.is_active_body_origin(variant_ref.origin) {
            return Ok(self
                .state
                .builder
                .local_enum_variant(variant_ref.local_enum_variant));
        }

        self.def_maps.local_enum_variant_data(variant_ref)
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        if self.is_active_body_origin(enum_def.origin) {
            return Ok(self
                .state
                .builder
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect());
        }

        self.def_maps.local_enum_variant_entries_for_enum(enum_def)
    }
}

impl<S> MacroDefinitionEnv for LocalDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, Self::Error> {
        let DefId::Local(def_ref) = def else {
            return Ok(None);
        };
        let Some(local_def) = self.local_def_data(def_ref)? else {
            return Ok(None);
        };

        let data = if self.is_active_body_origin(def_ref.origin) {
            self.state.builder.macro_definition(def_ref.local_def)
        } else {
            self.def_maps
                .def_map_for_origin(def_ref.origin)?
                .and_then(|def_map| def_map.macro_definition(def_ref.local_def))
        };
        let Some(data) = data else {
            return Ok(None);
        };

        Ok(MacroDefinitionView::new(def_ref, local_def, data))
    }
}

impl<S> CrateResolutionEnv for LocalDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.extern_root(crate_ref, name)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.root_module(crate_ref)
    }
}
