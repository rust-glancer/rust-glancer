//! Collects and finalizes body-local DefMap facts.
//!
//! Body scopes become synthetic modules. Direct declarations are collected first, then imports are
//! resolved in a fixed-point loop before the final DefMap is frozen.

use rg_ir_model::items::{
    Documentation, ImportAlias, ItemKind, ItemNode, ItemTreeId, ModuleSource,
};
use rg_ir_model::{
    BodyRef, DefId, DefMapRef, LocalDefRef, ModuleId, ModuleRef, TargetRef,
    hir::source::{BodyItemSourceRef, ItemSource},
};
use rg_ir_storage::{
    DefMap, DefMapBuilder, DefMapSource, ImportBinding, ImportData, ImportKind, ImportPath,
    ImportSourcePath, LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, ModuleData,
    ModuleOrigin, ModuleScope, ModuleScopeBuilder, Namespace, PathResolver, ScopeBinding,
    ScopeBindingOrigin, ScopeEntryRef, ScopeResolutionEnv, TargetResolutionEnv,
};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use crate::ResolvedBodyData;

pub(crate) struct BodyDefMapCollector<'body> {
    body_ref: BodyRef,
    body: &'body ResolvedBodyData,
    builder: DefMapBuilder,
    /// There might be more modules than scopes, so we need a mapping.
    /// Keys here are scope IDs, values are corresponding modules.
    modules_by_scope: Vec<ModuleId>,
    base_scopes: Vec<ModuleScopeBuilder>,
}

impl<'body> BodyDefMapCollector<'body> {
    pub fn new(body_ref: BodyRef, body: &'body ResolvedBodyData) -> Self {
        Self {
            body_ref,
            body,
            builder: DefMapBuilder::new_body(body_ref),
            modules_by_scope: Vec::with_capacity(body.scopes().len()),
            base_scopes: Vec::with_capacity(body.scopes().len()),
        }
    }

    /// Collects direct body-local scope facts. Imports are finalized in a separate fixed-point step.
    pub fn collect(mut self) -> BodyDefMapBuildState {
        // First, go through all the scopes and allocate synthetic modules.
        for (_, scope) in self.body.scopes_with_ids() {
            // Body scopes are synthetic modules. They carry lexical scope data, but they do not
            // correspond to Rust module declarations and lookup must treat them differently.
            let origin = ModuleOrigin::Synthetic {
                file_id: self.body.source().file_id,
                span: self.body.source().span,
            };

            // Note: we're going through scopes in order, so we process all the parents first.
            let parent = scope.parent.map(|parent| self.modules_by_scope[parent.0]);
            let module = self.alloc_module(ModuleData {
                name: None,
                name_span: None,
                docs: None,
                parent,
                children: Vec::new(),
                local_defs: Vec::new(),
                impls: Vec::new(),
                imports: Vec::new(),
                unresolved_imports: Vec::new(),
                scope: ModuleScope::default(),
                origin,
            });
            self.modules_by_scope.push(module);
        }

        // Second, go through all the items in each scope and collect them too.
        // Note that we are collecting _items_ from scopes, but here we do not
        // recurse: even if an item has a body, we do not start to analyze it.
        for (scope_id, scope) in self.body.scopes_with_ids() {
            let module = *self
                .modules_by_scope
                .get(scope_id.0)
                .expect("Must be provided");
            for item in &scope.source_items {
                self.collect_item(module, *item);
            }
        }

        BodyDefMapBuildState {
            body_ref: self.body_ref,
            builder: self.builder,
            base_scopes: self.base_scopes,
        }
    }

    fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        // Keep a mutable base scope next to every allocated module. The final def-map stores only
        // the frozen scope, but collection needs cheap direct-binding insertion.
        let module = self.builder.alloc_module(module);
        self.base_scopes.push(ModuleScopeBuilder::default());
        module
    }

    fn collect_item(&mut self, module: ModuleId, item_id: ItemTreeId) {
        let Some(item) = self.body.source_item(item_id) else {
            return;
        };

        match &item.kind {
            ItemKind::Module(module_item) => self.collect_module(module, item, module_item),
            ItemKind::Use(use_item) => self.collect_use(module, item_id, item, use_item),
            ItemKind::Impl(_) => self.collect_local_impl(module, item_id, item),
            ItemKind::MacroCall(_)
            | ItemKind::MacroDefinition(_)
            | ItemKind::ExternCrate(_)
            | ItemKind::ExternBlock
            | ItemKind::AsmExpr => {}
            _ => self.collect_local_def(module, item_id, item),
        }
    }

    fn collect_local_def(&mut self, module: ModuleId, item_id: ItemTreeId, item: &ItemNode) {
        let Some(kind) = LocalDefKind::from_item_tag(item.kind.tag()) else {
            return;
        };
        let Some(name) = item.name.clone() else {
            return;
        };
        let namespace = kind.namespace();

        let local_def = self.builder.alloc_local_def(LocalDefData {
            module,
            name: name.clone(),
            kind,
            visibility: item.visibility.clone(),
            source: self.item_source(item_id, item),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
        });
        self.builder
            .module_mut(module)
            .expect("module should exist for body local definition")
            .local_defs
            .push(local_def);
        self.base_scopes
            .get_mut(module.0)
            .expect("base scope should exist for body local definition")
            .insert_binding(
                &name,
                namespace,
                ScopeBinding {
                    def: DefId::Local(LocalDefRef {
                        origin: DefMapRef::Body(self.body_ref),
                        local_def,
                    }),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        origin: DefMapRef::Body(self.body_ref),
                        module,
                    },
                    origin: ScopeBindingOrigin::Direct,
                },
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
            ModuleSource::OutOfLine { definition_file } => ModuleOrigin::OutOfLine {
                declaration_file: item.file_id,
                declaration_span: item.span,
                definition_file: *definition_file,
            },
        };
        let docs = match &module_item.source {
            ModuleSource::Inline { .. } => {
                Documentation::concat(item.docs.clone(), module_item.inner_docs.clone())
            }
            ModuleSource::OutOfLine { .. } => item.docs.clone(),
        };

        let child = self.alloc_module(ModuleData {
            name: Some(module_name.clone()),
            name_span: item.name_span,
            docs,
            parent: Some(parent),
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin,
        });
        self.builder
            .module_mut(parent)
            .expect("parent module should exist for body child module")
            .children
            .push((module_name.clone(), child));
        self.base_scopes
            .get_mut(parent.0)
            .expect("base scope should exist for body child module")
            .insert_binding(
                &module_name,
                Namespace::Types,
                ScopeBinding {
                    def: DefId::Module(ModuleRef {
                        origin: DefMapRef::Body(self.body_ref),
                        module: child,
                    }),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        origin: DefMapRef::Body(self.body_ref),
                        module: parent,
                    },
                    origin: ScopeBindingOrigin::Direct,
                },
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
            let path = ImportPath::from_use_path(&import.path);
            if path.segments.is_empty() {
                continue;
            }

            let import = self.builder.alloc_import(ImportData {
                module,
                visibility: item.visibility.clone(),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                source_path: ImportSourcePath::from_use_path(&import.path),
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { span, .. } => Some(*span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: self.item_source(item_id, item),
                import_index,
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
pub(crate) struct BodyDefMapBuildState {
    body_ref: BodyRef,
    builder: DefMapBuilder,
    base_scopes: Vec<ModuleScopeBuilder>,
}

impl BodyDefMapBuildState {
    pub(crate) fn finalize<S>(mut self, def_maps: S) -> Result<DefMap, PackageStoreError>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        let final_scopes = self.resolve_import_scopes(def_maps)?;
        let unresolved_imports = self.collect_unresolved_imports(def_maps, &final_scopes)?;

        for (module_idx, scope) in final_scopes.iter().enumerate() {
            let module = self
                .builder
                .module_mut(ModuleId(module_idx))
                .expect("module should exist for body scope freeze");
            module.scope = scope.freeze();
            module.unresolved_imports = unresolved_imports
                .get(module_idx)
                .expect("unresolved imports should exist for every body module")
                .clone();
        }

        Ok(self.builder.build())
    }

    fn resolve_import_scopes<S>(
        &self,
        def_maps: S,
    ) -> Result<Vec<ModuleScopeBuilder>, PackageStoreError>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        let mut current_scopes = self.base_scopes.clone();

        loop {
            let mut next_scopes = self.base_scopes.clone();
            let env = BodyDefMapFinalizationEnv {
                def_maps,
                state: self,
                current_scopes: &current_scopes,
            };
            self.apply_imports(&env, &mut next_scopes)?;

            if next_scopes == current_scopes {
                return Ok(current_scopes);
            }

            current_scopes = next_scopes;
        }
    }

    fn apply_imports<S>(
        &self,
        env: &BodyDefMapFinalizationEnv<'_, S>,
        next_scopes: &mut [ModuleScopeBuilder],
    ) -> Result<(), PackageStoreError>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        let resolver = PathResolver::new(env);
        for import in self.builder.partial().imports() {
            let importing_module = self.importing_module(import.module);
            match import.kind {
                ImportKind::Glob => {
                    let source_modules =
                        resolver.import_modules_from_module(importing_module, &import.path)?;

                    for source_module in source_modules {
                        let source_scope =
                            resolver.visible_scope(importing_module, source_module)?;
                        let target_scope = next_scopes
                            .get_mut(import.module.0)
                            .expect("target scope should exist for body import");

                        for (name, entry) in source_scope.entries() {
                            target_scope.copy_visible_bindings(
                                name,
                                entry,
                                import.visibility.clone(),
                                importing_module,
                            );
                        }
                    }
                }
                ImportKind::Named | ImportKind::SelfImport => {
                    let resolved_defs =
                        resolver.import_defs_from_module(importing_module, &import.path)?;
                    let Some(binding_name) = import.binding_name() else {
                        continue;
                    };
                    let target_scope = next_scopes
                        .get_mut(import.module.0)
                        .expect("target scope should exist for body import");

                    for resolved_def in resolved_defs {
                        let Some(namespace) = resolver.namespace_for_def(resolved_def)? else {
                            continue;
                        };
                        target_scope.insert_binding(
                            &binding_name,
                            namespace,
                            ScopeBinding {
                                def: resolved_def,
                                visibility: import.visibility.clone(),
                                owner: importing_module,
                                origin: ScopeBindingOrigin::Import,
                            },
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn collect_unresolved_imports<S>(
        &self,
        def_maps: S,
        final_scopes: &[ModuleScopeBuilder],
    ) -> Result<Vec<Vec<rg_ir_model::ImportId>>, PackageStoreError>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        let mut module_imports = vec![Vec::new(); self.builder.partial().module_count()];
        let env = BodyDefMapFinalizationEnv {
            def_maps,
            state: self,
            current_scopes: final_scopes,
        };
        let resolver = PathResolver::new(&env);

        for (import_id, import) in self.builder.partial().imports_with_ids() {
            let importing_module = self.importing_module(import.module);
            let is_unresolved = match import.kind {
                ImportKind::Glob => resolver
                    .import_modules_from_module(importing_module, &import.path)?
                    .is_empty(),
                ImportKind::Named | ImportKind::SelfImport => resolver
                    .import_defs_from_module(importing_module, &import.path)?
                    .is_empty(),
            };
            if is_unresolved {
                module_imports
                    .get_mut(import.module.0)
                    .expect("import module should exist while collecting body unresolved imports")
                    .push(import_id);
            }
        }

        Ok(module_imports)
    }

    fn importing_module(&self, module: ModuleId) -> ModuleRef {
        ModuleRef {
            origin: DefMapRef::Body(self.body_ref),
            module,
        }
    }
}

struct BodyDefMapFinalizationEnv<'state, S> {
    def_maps: S,
    state: &'state BodyDefMapBuildState,
    current_scopes: &'state [ModuleScopeBuilder],
}

impl<S> BodyDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    fn is_active_body_origin(&self, origin: DefMapRef) -> bool {
        origin == DefMapRef::Body(self.state.body_ref)
    }
}

impl<S> ScopeResolutionEnv for BodyDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    type Error = PackageStoreError;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error> {
        if self.is_active_body_origin(module_ref.origin) {
            return Ok(self.state.builder.partial().module(module_ref.module));
        }

        Ok(self
            .def_maps
            .def_map_for_origin(module_ref.origin)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error> {
        if self.is_active_body_origin(module_ref.origin) {
            return Ok(self
                .current_scopes
                .get(module_ref.module.0)
                .and_then(|scope| scope.entry(name)));
        }

        Ok(self
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
                .current_scopes
                .get(module_ref.module.0)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }

        Ok(self
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
            return Ok(self
                .state
                .builder
                .partial()
                .local_def(local_def_ref.local_def));
        }

        Ok(self
            .def_maps
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, Self::Error> {
        if self.is_active_body_origin(local_def_ref.origin) {
            return Ok(self
                .state
                .builder
                .partial()
                .macro_definition(local_def_ref.local_def));
        }

        Ok(self
            .def_maps
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }
}

impl<S> TargetResolutionEnv for BodyDefMapFinalizationEnv<'_, S>
where
    S: DefMapSource<Error = PackageStoreError> + Copy,
{
    fn extern_root(&self, target: TargetRef, name: &str) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.def_maps.root_module(target)
    }
}
