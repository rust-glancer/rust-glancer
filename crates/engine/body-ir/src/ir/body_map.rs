use anyhow::Context as _;
use rg_arena::Arena;
use rg_def_map::{
    DefMap, DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
    LocalDefData, LocalDefKind, LocalImplData, ModuleData, ModuleOrigin, ModuleScope,
    ModuleScopeBuilder, Namespace, ScopeBinding, ScopeBindingOrigin,
};
use rg_ir_model::{
    BodyRef, DefId, DefMapRef, LocalDefRef, ModuleId, ModuleRef,
    hir::source::{BodyItemSourceRef, ItemSource, ItemSourceKind},
};
use rg_item_tree::{Documentation, ImportAlias, ItemKind, ItemNode, ItemTreeId, ModuleSource};
use rg_semantic_ir::{ItemStore, ItemStoreLowerer, ItemStoreSourceReader};

use super::BodyData;

/// Item-tree-shaped source payloads declared inside one function body.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodySourceItems {
    pub(crate) items: Arena<ItemTreeId, ItemNode>,
}

impl BodySourceItems {
    pub fn item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item)
    }

    pub fn items(&self) -> &[ItemNode] {
        self.items.as_slice()
    }

    pub(crate) fn alloc(&mut self, item: ItemNode) -> ItemTreeId {
        self.items.alloc(item)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        for item in self.items.iter_mut() {
            item.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}

pub(crate) struct BodyDefMapCollector<'body> {
    body_ref: BodyRef,
    body: &'body BodyData,
    builder: DefMapBuilder,
    /// There might be more modules than scopes, so we need a mapping.
    /// Keys here are scope IDs, values are corresponding modules.
    modules_by_scope: Vec<ModuleId>,
    base_scopes: Vec<ModuleScopeBuilder>,
}

impl<'body> BodyDefMapCollector<'body> {
    pub fn new(target_def_map: &DefMap, body_ref: BodyRef, body: &'body BodyData) -> Self {
        Self {
            body_ref,
            body,
            builder: target_def_map.child(body_ref),
            modules_by_scope: Vec::with_capacity(body.scopes.len()),
            base_scopes: Vec::with_capacity(body.scopes.len()),
        }
    }

    /// Collects the body item tree into a body-local DefMap.
    pub fn collect(mut self) -> DefMap {
        // First, go through all the scopes and allocate synthetic modules.
        for (_, scope) in self.body.scopes.iter_with_ids() {
            // Body scopes are synthetic modules. They carry lexical scope data, but they do not
            // correspond to Rust module declarations and lookup must treat them differently.
            let origin = ModuleOrigin::Synthetic {
                file_id: self.body.source.file_id,
                span: self.body.source.span,
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
        for (scope_id, scope) in self.body.scopes.iter_with_ids() {
            let module = *self
                .modules_by_scope
                .get(scope_id.0)
                .expect("Must be provided");
            for item in &scope.source_items {
                self.collect_item(module, *item);
            }
        }

        // TODO: For now, we do not have any kind of import resolution / fixed loop,
        // as they are much less relevant for bodies. It might be relevant eventually
        // but for now it's omitted for simplicity.
        self.freeze_scopes();
        self.builder.build()
    }

    fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        // Keep a mutable base scope next to every allocated module. The final def-map stores only
        // the frozen scope, but collection needs cheap direct-binding insertion.
        let module = self.builder.alloc_module(module);
        self.base_scopes.push(ModuleScopeBuilder::default());
        module
    }

    fn freeze_scopes(&mut self) {
        for (module_idx, base_scope) in self.base_scopes.iter().enumerate() {
            self.builder
                .module_mut(ModuleId(module_idx))
                .expect("module should exist for body scope freeze")
                .scope = base_scope.freeze();
        }
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

pub(crate) struct BodyItemStoreCollector<'body> {
    body: &'body BodyData,
    def_map: &'body DefMap,
}

impl<'body> BodyItemStoreCollector<'body> {
    pub fn new(body: &'body BodyData, def_map: &'body DefMap) -> Self {
        Self { body, def_map }
    }

    /// Lowers body-local DefMap entries into semantic item-shaped shadow storage.
    pub fn collect(self) -> ItemStore {
        let reader = BodyItemStoreSourceReader {
            body: self.body,
            def_map: self.def_map,
        };
        ItemStoreLowerer::new(self.def_map, reader)
            .lower()
            .expect("body item store should lower from collected body source items")
    }
}

// Allows to reuse the generic `ItemStoreLowerer` by providing an interface to read
// item tree-like storage.
struct BodyItemStoreSourceReader<'body> {
    body: &'body BodyData,
    def_map: &'body DefMap,
}

impl<'body> ItemStoreSourceReader<'body> for BodyItemStoreSourceReader<'body> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'body ItemNode> {
        let (DefMapRef::Body(body_ref), ItemSourceKind::Body(source)) =
            (self.def_map.own_ref(), source.kind)
        else {
            anyhow::bail!("body item store source should point to body source item");
        };

        if source.body != body_ref {
            anyhow::bail!("body item store source should belong to this body");
        }

        self.body.source_item(source.item).with_context(|| {
            format!(
                "while attempting to fetch body source item {:?}",
                source.item
            )
        })
    }
}
