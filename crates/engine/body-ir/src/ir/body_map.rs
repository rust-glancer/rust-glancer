use rg_arena::Arena;
use rg_def_map::{
    DefMap, DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
    LocalDefData, LocalDefKind, LocalImplData, ModuleData, ModuleOrigin, ModuleScope,
};
use rg_ir_model::{
    AssocItemId, BodyRef, ConstId, DefMapRef, FunctionId, ItemId, ItemOwner, LocalDefRef,
    LocalImplRef, ModuleId, ModuleRef, StaticId, TraitId, TypeAliasId,
    hir::{
        items::{
            ConstData, EnumData, FunctionData, ImplData, StaticData, StructData, TraitData,
            TypeAliasData, UnionData,
        },
        signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
        source::{BodyItemSourceRef, ItemSource, ItemSourceKind},
    },
};
use rg_item_tree::{
    ConstItem, Documentation, FunctionItem, ImplItem, ImportAlias, ItemKind, ItemNode, ItemTreeId,
    ModuleSource, StaticItem, TraitItem, TypeAliasItem,
};
use rg_semantic_ir::{ItemStore, ItemStoreBuilder};
use rg_text::Name;

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
}

impl<'body> BodyDefMapCollector<'body> {
    pub fn new(target_def_map: &DefMap, body_ref: BodyRef, body: &'body BodyData) -> Self {
        Self {
            body_ref,
            body,
            builder: target_def_map.child(body_ref),
            modules_by_scope: Vec::with_capacity(body.scopes.len()),
        }
    }

    /// Collects the body item tree into a body-local DefMap.
    pub fn collect(mut self) -> DefMap {
        // First, go through all the scopes and allocate synthetic modules.
        for (scope_id, scope) in self.body.scopes.iter_with_ids() {
            let origin = if scope_id == self.body.param_scope {
                // We use param scope as root; it obviously cannot contain
                // any items, so this module will always be empty. Items are only
                // expected to be in the synthetic modules that will be children
                // of this one.
                ModuleOrigin::Root {
                    file_id: self.body.source.file_id,
                }
            } else {
                // Body scopes are synthetic modules. They have no source declaration of their own, so the
                // containing function span acts as the stable source marker until lookup starts using them.
                ModuleOrigin::Inline {
                    declaration_file: self.body.source.file_id,
                    declaration_span: self.body.source.span,
                }
            };

            // Note: we're going through scopes in order, so we process all the parents first.
            let parent = scope.parent.map(|parent| self.modules_by_scope[parent.0]);
            let module = self.builder.alloc_module(ModuleData {
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
        self.builder.build()
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

        let local_def = self.builder.alloc_local_def(LocalDefData {
            module,
            name,
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

        let child = self.builder.alloc_module(ModuleData {
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
            .push((module_name, child));

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
    items: ItemStoreBuilder,
}

impl<'body> BodyItemStoreCollector<'body> {
    pub fn new(body: &'body BodyData, def_map: &'body DefMap) -> Self {
        Self {
            body,
            def_map,
            items: ItemStoreBuilder::for_def_map(def_map),
        }
    }

    /// Lowers body-local DefMap entries into semantic item-shaped shadow storage.
    pub fn collect(mut self) -> ItemStore {
        // Go through the definitions in this body.
        for local_def_ref in self.def_map.local_def_refs() {
            let local_def = self
                .def_map
                .local_def(local_def_ref.local_def)
                .expect("body local definition should exist while lowering item store");
            let Some(item) = self.item(local_def.source) else {
                continue;
            };
            let owner = ModuleRef {
                origin: self.def_map.own_ref(),
                module: local_def.module,
            };

            if let Some(item_id) =
                self.lower_local_item(local_def_ref, local_def.source, owner, item)
            {
                self.items.set_local_item(local_def_ref.local_def, item_id);
            }
        }

        // Go through the impls in this body.
        for local_impl_ref in self.def_map.local_impl_refs() {
            let local_impl = self
                .def_map
                .local_impl(local_impl_ref.local_impl)
                .expect("body local impl should exist while lowering item store");
            let Some(item) = self.item(local_impl.source) else {
                continue;
            };
            let owner = ModuleRef {
                origin: self.def_map.own_ref(),
                module: local_impl.module,
            };

            if let ItemKind::Impl(impl_item) = &item.kind {
                self.lower_impl(local_impl_ref, local_impl.source, owner, impl_item);
            }
        }

        self.items.build()
    }

    fn item(&self, source: ItemSource) -> Option<&'body ItemNode> {
        let (DefMapRef::Body(body_ref), ItemSourceKind::Body(source)) =
            (self.def_map.own_ref(), source.kind)
        else {
            return None;
        };

        if source.body != body_ref {
            return None;
        }

        self.body.source_item(source.item)
    }

    fn lower_local_item(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
    ) -> Option<ItemId> {
        match &item.kind {
            ItemKind::Const(const_item) => Some(ItemId::Const(self.lower_const(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                const_item,
            ))),
            ItemKind::Enum(enum_item) => {
                let id = self.items.enums.alloc(EnumData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: enum_item.generics.clone(),
                    variants: enum_item.variants.clone(),
                });
                Some(ItemId::Enum(id))
            }
            ItemKind::Function(function_item) => Some(ItemId::Function(self.lower_function(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                function_item,
            ))),
            ItemKind::Static(static_item) => Some(ItemId::Static(self.lower_static(
                local_def,
                source,
                owner,
                item,
                static_item,
            ))),
            ItemKind::Struct(struct_item) => {
                let id = self.items.structs.alloc(StructData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: struct_item.generics.clone(),
                    fields: struct_item.fields.clone(),
                });
                Some(ItemId::Struct(id))
            }
            ItemKind::Trait(trait_item) => Some(ItemId::Trait(
                self.lower_trait(local_def, source, owner, item, trait_item),
            )),
            ItemKind::TypeAlias(type_alias) => Some(ItemId::TypeAlias(self.lower_type_alias(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                type_alias,
            ))),
            ItemKind::Union(union_item) => {
                let id = self.items.unions.alloc(UnionData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: union_item.generics.clone(),
                    fields: union_item.fields.clone(),
                });
                Some(ItemId::Union(id))
            }
            _ => None,
        }
    }

    fn lower_trait(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
        trait_item: &TraitItem,
    ) -> TraitId {
        let trait_id = self.items.traits.alloc(TraitData {
            local_def,
            source,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            generics: trait_item.generics.clone(),
            super_traits: trait_item.super_traits.clone(),
            items: Vec::new(),
            is_unsafe: trait_item.is_unsafe,
        });
        let assoc_items =
            self.lower_assoc_items(source, &trait_item.items, ItemOwner::Trait(trait_id));
        self.items.traits[trait_id].items = assoc_items;
        trait_id
    }

    fn lower_impl(
        &mut self,
        local_impl: LocalImplRef,
        source: ItemSource,
        owner: ModuleRef,
        impl_item: &ImplItem,
    ) {
        let impl_id = self.items.impls.alloc(ImplData {
            local_impl,
            source,
            owner,
            generics: impl_item.generics.clone(),
            trait_ref: impl_item.trait_ref.clone(),
            self_ty: impl_item.self_ty.clone(),
            resolved_self_tys: Vec::new(),
            resolved_trait_refs: Vec::new(),
            items: Vec::new(),
            is_unsafe: impl_item.is_unsafe,
        });
        let assoc_items =
            self.lower_assoc_items(source, &impl_item.items, ItemOwner::Impl(impl_id));
        self.items.impls[impl_id].items = assoc_items;
    }

    fn lower_assoc_items(
        &mut self,
        parent_source: ItemSource,
        item_ids: &[ItemTreeId],
        owner: ItemOwner,
    ) -> Vec<AssocItemId> {
        let mut assoc_items = Vec::new();
        for item_id in item_ids {
            let source = parent_source.with_item(*item_id);
            let Some(item) = self.item(source) else {
                continue;
            };

            match &item.kind {
                ItemKind::Const(const_item) => {
                    assoc_items.push(AssocItemId::Const(
                        self.lower_const(None, source, owner, item, const_item),
                    ));
                }
                ItemKind::Function(function_item) => {
                    assoc_items.push(AssocItemId::Function(self.lower_function(
                        None,
                        source,
                        owner,
                        item,
                        function_item,
                    )));
                }
                ItemKind::TypeAlias(type_alias) => {
                    assoc_items.push(AssocItemId::TypeAlias(
                        self.lower_type_alias(None, source, owner, item, type_alias),
                    ));
                }
                _ => {}
            }
        }

        assoc_items
    }

    fn lower_function(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &FunctionItem,
    ) -> FunctionId {
        self.items.functions.alloc(FunctionData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: FunctionSignature::from_item(declaration),
        })
    }

    fn lower_type_alias(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &TypeAliasItem,
    ) -> TypeAliasId {
        self.items.type_aliases.alloc(TypeAliasData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: TypeAliasSignature::from_item(declaration),
        })
    }

    fn lower_const(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &ConstItem,
    ) -> ConstId {
        self.items.consts.alloc(ConstData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: ConstSignature::from_item(declaration),
        })
    }

    fn lower_static(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
        declaration: &StaticItem,
    ) -> StaticId {
        self.items.statics.alloc(StaticData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            ty: declaration.ty.clone(),
            mutability: declaration.mutability,
        })
    }
}
