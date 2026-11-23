//! Builds semantic item stores from DefMap identities and source declarations.
//!
//! Crate and body-local declarations use different source stores. The reader lets both use
//! the same lowering rules.

use std::marker::PhantomData;

use anyhow::Context as _;
use rg_def_map::{DefMap, ItemSource, LocalDefKind};
use rg_ir_model::{
    AssocItemId, ItemId, ItemOwner, LocalDefRef, LocalImplRef, ModuleRef, StaticId, TraitId,
};
use rg_item_tree::{
    ConstItem, FunctionItem, ImplItem, ItemKind, ItemNode, StaticItem, TraitItem, TypeAliasItem,
    UserFacingAttrs,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use crate::item::{
    AssociatedItemStoreBuilder, ConstData, ConstSignature, EnumData, FunctionData,
    FunctionSignature, ImplData, ItemStore, ItemStoreBuilder, StaticData, StructData, TraitData,
    TypeAliasData, TypeAliasSignature, UnionData,
};

/// Reads item-tree-shaped payloads from the storage layer named by an `ItemSource`.
pub trait ItemStoreSourceReader<'item> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'item ItemNode>;
}

/// Lowers a collected DefMap into item-shaped semantic storage.
///
/// This lowerer constructs scope declarations and owner headers. Member lowering is a separate
/// operation, whether a saved query performs it lazily or a body collector performs it eagerly.
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

    /// Lower module declarations and trait/impl headers without constructing associated stores.
    /// A saved target can publish these headers while each member store remains unrequested.
    pub fn lower(mut self) -> anyhow::Result<ItemStore> {
        self.lower_scope()?;
        Ok(self.items.build())
    }

    fn lower_scope(&mut self) -> anyhow::Result<()> {
        // DefMap local definitions provide the stable item identity. The source reader supplies the
        // syntax-shaped payload from Cargo-target item trees, generated sources, or body-local
        // arenas.
        for local_def_ref in self.def_map.local_def_refs() {
            let local_def = self
                .def_map
                .local_def(local_def_ref.local_def)
                .expect("local definition ref should be produced by this def map");

            // A macro definition can point at function-shaped syntax when it is the exported
            // identity of a procedural macro. Its LocalDef kind, not that shared source shape,
            // decides whether it owns a semantic item. The linked implementation function travels
            // through its own LocalDef and is lowered normally.
            if local_def.kind == LocalDefKind::MacroDefinition {
                continue;
            }

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
                self.lower_impl(
                    local_impl_ref,
                    local_impl.source,
                    owner,
                    impl_item,
                    item.user_facing_attrs,
                );
            }
        }

        Ok(())
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
        let item_id = match &item.kind {
            ItemKind::Const(const_item) => {
                let data = ConstData::from_item(
                    Some(local_def),
                    source,
                    ItemOwner::Module(owner),
                    item,
                    const_item,
                    item.user_facing_attrs,
                );
                ItemId::Const(self.items.consts.alloc(data))
            }
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
                ItemId::Enum(id)
            }
            ItemKind::Function(fn_def) => {
                let data = FunctionData::from_item(
                    Some(local_def),
                    source,
                    ItemOwner::Module(owner),
                    item,
                    fn_def,
                    item.user_facing_attrs,
                );
                ItemId::Function(self.items.functions.alloc(data))
            }
            ItemKind::Static(static_item) => {
                ItemId::Static(self.lower_static(local_def, source, owner, item, static_item))
            }
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
                ItemId::Struct(id)
            }
            ItemKind::Trait(trait_item) => {
                ItemId::Trait(self.lower_trait(local_def, source, owner, item, trait_item))
            }
            ItemKind::TypeAlias(type_alias) => {
                let data = TypeAliasData::from_item(
                    Some(local_def),
                    source,
                    ItemOwner::Module(owner),
                    item,
                    type_alias,
                    item.user_facing_attrs,
                );
                ItemId::TypeAlias(self.items.type_aliases.alloc(data))
            }
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
                ItemId::Union(id)
            }
            _ => return None,
        };
        self.items
            .register_lang_item(item.lang_item, item_id.semantic_ref(self.def_map.own_ref()));
        Some(item_id)
    }

    fn lower_trait(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
        trait_item: &TraitItem,
    ) -> TraitId {
        // Allocate the header in the enclosing scope first. Its ID also names the member store,
        // so members can point back to this header for their inherited generics.
        self.items.traits.alloc(TraitData {
            local_def,
            source,
            owner,
            name: item.name.clone().unwrap_or_else(Name::missing),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            generics: trait_item.generics.clone(),
            super_traits: trait_item.super_traits.clone(),
            is_unsafe: trait_item.is_unsafe,
            is_auto: trait_item.is_auto,
        })
    }

    fn lower_impl(
        &mut self,
        local_impl: LocalImplRef,
        source: ItemSource,
        owner: ModuleRef,
        impl_item: &ImplItem,
        user_facing_attrs: UserFacingAttrs,
    ) {
        // Impl header paths are stored as syntax here; a separate pass can fill resolved self/trait
        // ids once all stores needed by the chosen resolution scope exist.
        self.items.impls.alloc(ImplData {
            user_facing_attrs,
            local_impl,
            source,
            owner,
            generics: impl_item.generics.clone(),
            trait_ref: impl_item.trait_ref.clone(),
            self_ty: impl_item.self_ty.clone(),
            resolved_self_ty: ExpectedUnique::new(),
            resolved_trait_ref: ExpectedUnique::new(),
            is_unsafe: impl_item.is_unsafe,
            polarity: impl_item.polarity,
        });
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
            location: item.location,
            name_location: item.name_location,
            owner,
            name: item.name.clone().unwrap_or_else(Name::missing),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            ty: declaration.ty.clone(),
            mutability: declaration.mutability,
            initializer_location: declaration.initializer_location,
        })
    }
}

impl FunctionData {
    fn from_item(
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &FunctionItem,
        user_facing_attrs: UserFacingAttrs,
    ) -> Self {
        Self {
            user_facing_attrs,
            local_def,
            source,
            location: item.location,
            name_location: item.name_location,
            owner,
            name: item.name.clone().unwrap_or_else(Name::missing),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: FunctionSignature::from_item(declaration),
        }
    }
}

impl TypeAliasData {
    fn from_item(
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &TypeAliasItem,
        user_facing_attrs: UserFacingAttrs,
    ) -> Self {
        Self {
            user_facing_attrs,
            local_def,
            source,
            location: item.location,
            name_location: item.name_location,
            owner,
            name: item.name.clone().unwrap_or_else(Name::missing),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: TypeAliasSignature::from_item(declaration),
        }
    }
}

impl ConstData {
    fn from_item(
        local_def: Option<LocalDefRef>,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &ConstItem,
        user_facing_attrs: UserFacingAttrs,
    ) -> Self {
        Self {
            user_facing_attrs,
            local_def,
            source,
            location: item.location,
            name_location: item.name_location,
            owner,
            name: item.name.clone().unwrap_or_else(Name::missing),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: ConstSignature::from_item(declaration),
            initializer_location: declaration.initializer_location,
        }
    }
}

impl AssociatedItemStoreBuilder {
    /// Allocate IDs only for declarations. Macro calls consume no slots; their expanded
    /// declarations must be supplied separately. Members have no module-level binding, so the
    /// lowered data has no `local_def` even though it has its own semantic member ID.
    /// Keep an impl's presentation attributes on its members so completion can
    /// check them without reopening the owner or changing Rust name resolution.
    pub fn lower(
        &mut self,
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        impl_attrs: UserFacingAttrs,
    ) -> Option<AssocItemId> {
        let attrs = item.user_facing_attrs.merge(impl_attrs);
        Some(match &item.kind {
            ItemKind::Function(declaration) => {
                let data = FunctionData::from_item(None, source, owner, item, declaration, attrs);
                AssocItemId::Function(self.alloc_function(data))
            }
            ItemKind::Const(declaration) => {
                let data = ConstData::from_item(None, source, owner, item, declaration, attrs);
                AssocItemId::Const(self.alloc_const(data))
            }
            ItemKind::TypeAlias(declaration) => {
                let data = TypeAliasData::from_item(None, source, owner, item, declaration, attrs);
                AssocItemId::TypeAlias(self.alloc_type_alias(data))
            }
            _ => return None,
        })
    }
}
