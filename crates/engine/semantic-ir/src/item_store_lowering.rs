use std::marker::PhantomData;

use anyhow::Context as _;

use rg_def_map::DefMap;
use rg_ir_model::{
    AssocItemId, ConstId, FunctionId, ItemId, ItemOwner, LocalDefRef, LocalImplRef, ModuleRef,
    StaticId, TraitId, TypeAliasId,
    hir::{
        items::{
            ConstData, EnumData, FunctionData, ImplData, StaticData, StructData, TraitData,
            TypeAliasData, UnionData,
        },
        signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
        source::ItemSource,
    },
};
use rg_item_tree::{
    ConstItem, FunctionItem, ImplItem, ItemKind, ItemNode, ItemTreeId, StaticItem, TraitItem,
    TypeAliasItem,
};
use rg_text::Name;

use crate::{ItemStore, ItemStoreBuilder};

/// Reads item-tree-shaped payloads from the storage layer named by an `ItemSource`.
pub trait ItemStoreSourceReader<'item> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'item ItemNode>;
}

/// Lowers a collected DefMap into item-shaped semantic storage.
///
/// This structure is inetnionally generic enough to allow reuse across
/// both semantic and body layers.
pub struct ItemStoreLowerer<'def_map, 'item, R> {
    def_map: &'def_map DefMap,
    reader: R,
    items: ItemStoreBuilder,
    item_lifetime: PhantomData<&'item ItemNode>,
}

impl<'def_map, 'item, R> ItemStoreLowerer<'def_map, 'item, R>
where
    R: ItemStoreSourceReader<'item>,
{
    pub fn new(def_map: &'def_map DefMap, reader: R) -> Self {
        Self {
            def_map,
            reader,
            items: ItemStoreBuilder::new(def_map.own_ref(), def_map.local_defs().len()),
            item_lifetime: PhantomData,
        }
    }

    pub fn lower(mut self) -> anyhow::Result<ItemStore> {
        // DefMap local definitions provide the stable item identity. The source reader supplies the
        // syntax-shaped payload from target item trees, generated sources, or body-local arenas.
        for local_def_ref in self.def_map.local_def_refs() {
            let local_def = self
                .def_map
                .local_def(local_def_ref.local_def)
                .expect("local definition ref should be produced by this def map");
            let item = self.reader.item(local_def.source).with_context(|| {
                format!(
                    "while attempting to read source item for local definition {:?}",
                    local_def_ref.local_def,
                )
            })?;
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

        // Impl blocks are not namespace bindings, so they travel through a separate def-map table
        // and are lowered after named items have their semantic ids.
        for local_impl_ref in self.def_map.local_impl_refs() {
            let local_impl = self
                .def_map
                .local_impl(local_impl_ref.local_impl)
                .expect("local impl ref should be produced by this def map");
            let item = self.reader.item(local_impl.source).with_context(|| {
                format!(
                    "while attempting to read source item for local impl {:?}",
                    local_impl_ref.local_impl,
                )
            })?;
            let owner = ModuleRef {
                origin: self.def_map.own_ref(),
                module: local_impl.module,
            };

            if let ItemKind::Impl(impl_item) = &item.kind {
                self.lower_impl(local_impl_ref, local_impl.source, owner, impl_item);
            }
        }

        Ok(self.items.build())
    }

    fn lower_local_item(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
    ) -> Option<ItemId> {
        // Imports, modules, and unsupported syntax already did their def-map work. Item stores keep
        // declarations that carry signature facts or item identities for queries.
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
        // Allocate first so associated items can point back at this trait as their owner.
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
        // Impl header paths are stored as syntax here; a separate pass can fill resolved self/trait
        // ids once all stores needed by the chosen resolution scope exist.
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

        // Associated items share the same stores as module items, but they do not have def-map
        // local definitions because they are reached through their trait/impl owner.
        for item_id in item_ids {
            let source = parent_source.with_item(*item_id);
            let Ok(item) = self.reader.item(source) else {
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
