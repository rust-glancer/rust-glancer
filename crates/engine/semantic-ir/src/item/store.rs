use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{
    ConstId, ConstRef, CrateRef, DefMapRef, EnumId, FunctionId, FunctionRef, ImplId, ImplRef,
    ItemId, ItemStoreId, ItemStoreRef, LocalDefId, SemanticItemRef, StaticId, StaticRef, StructId,
    TraitDefRef, TraitId, TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
};
use rg_item_tree::LangItem;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{
    AssociatedItemStore, ConstData, EnumData, FunctionData, ImplData, SemanticItemView, StaticData,
    StructData, TraitData, TypeAliasData, UnionData, lang_item::LangItemIndex,
    view::SemanticItemData,
};

#[derive(Debug)]
pub struct ItemStoreBuilder {
    // DefMap this item store corresponds to.
    origin: DefMapRef,

    // Mapping from local def ID to semantic item ID.
    pub local_items: Arena<LocalDefId, Option<ItemId>>,

    pub structs: Arena<StructId, StructData>,
    pub unions: Arena<UnionId, UnionData>,
    pub enums: Arena<EnumId, EnumData>,
    pub traits: Arena<TraitId, TraitData>,
    pub impls: Arena<ImplId, ImplData>,
    pub functions: Arena<FunctionId, FunctionData>,
    pub type_aliases: Arena<TypeAliasId, TypeAliasData>,
    pub consts: Arena<ConstId, ConstData>,
    pub statics: Arena<StaticId, StaticData>,
    members: HashMap<ItemStoreId, AssociatedItemStore>,
    lang_items: LangItemIndex,
}

impl ItemStoreBuilder {
    pub fn new(origin: DefMapRef, local_def_count: usize) -> Self {
        Self {
            origin,
            local_items: {
                let mut local_items = Arena::new();
                local_items.resize_with(local_def_count, || None);
                local_items
            },
            structs: Arena::default(),
            unions: Arena::default(),
            enums: Arena::default(),
            traits: Arena::default(),
            impls: Arena::default(),
            functions: Arena::default(),
            type_aliases: Arena::default(),
            consts: Arena::default(),
            statics: Arena::default(),
            members: HashMap::new(),
            lang_items: LangItemIndex::default(),
        }
    }

    pub fn set_local_item(&mut self, local_def: LocalDefId, item: ItemId) {
        let slot = self
            .local_items
            .get_mut(local_def)
            .expect("local item slot should exist while building semantic IR");
        *slot = Some(item);
    }

    /// Records a compiler-known identity after its typed semantic target has been allocated.
    pub(crate) fn register_lang_item(
        &mut self,
        lang_item: Option<LangItem>,
        item: SemanticItemRef,
    ) {
        if let Some(lang_item) = lang_item {
            self.lang_items.insert(lang_item, item);
        }
    }

    pub fn set_members(&mut self, id: ItemStoreId, members: AssociatedItemStore) {
        self.members.insert(id, members);
    }

    pub fn build(self) -> ItemStore {
        ItemStore {
            origin: self.origin,
            local_items: self.local_items,
            structs: self.structs,
            unions: self.unions,
            enums: self.enums,
            traits: self.traits,
            impls: self.impls,
            functions: self.functions,
            type_aliases: self.type_aliases,
            consts: self.consts,
            statics: self.statics,
            members: self.members,
            lang_items: self.lang_items,
        }
    }
}

/// Declarations from one crate or body scope, including the member stores for its traits and impls.
///
/// ```text
/// impl<T> Bag<T> {
///     fn first(&self) -> &T { ... }
/// }
/// ```
/// `impls` stores the [`ImplData`] for `impl<T> Bag<T>`. A separate [`AssociatedItemStore`], keyed
/// by that block's `ImplId`, stores `first`. Lookup can inspect the header to decide whether the
/// impl applies to a receiver, then ask for the declarations inside it.
///
/// The `functions`, `type_aliases` and `consts` arenas here hold free declarations only. Associated
/// declarations live in `members`; their references record which store owns their local ID.
/// Lowering fills both the scope arenas and the member stores before publishing this `ItemStore`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemStore {
    // DefMap this item store corresponds to.
    origin: DefMapRef,

    // Mapping from local def ID to semantic item ID.
    local_items: Arena<LocalDefId, Option<ItemId>>,

    structs: Arena<StructId, StructData>,
    unions: Arena<UnionId, UnionData>,
    enums: Arena<EnumId, EnumData>,
    traits: Arena<TraitId, TraitData>,
    impls: Arena<ImplId, ImplData>,
    functions: Arena<FunctionId, FunctionData>,
    type_aliases: Arena<TypeAliasId, TypeAliasData>,
    consts: Arena<ConstId, ConstData>,
    statics: Arena<StaticId, StaticData>,
    members: HashMap<ItemStoreId, AssociatedItemStore>,
    lang_items: LangItemIndex,
}

impl ItemStore {
    pub fn origin(&self) -> DefMapRef {
        self.origin
    }

    pub fn crate_ref(&self) -> CrateRef {
        self.origin.origin_crate()
    }

    pub fn structs(&self) -> &Arena<StructId, StructData> {
        &self.structs
    }

    pub fn traits(&self) -> &Arena<TraitId, TraitData> {
        &self.traits
    }

    pub fn impls(&self) -> &Arena<ImplId, ImplData> {
        &self.impls
    }

    /// Applies resolved header facts after impl item identities are already allocated.
    pub fn set_impl_header_facts(
        &mut self,
        id: ImplId,
        resolved_self_ty: ExpectedUnique<TypeDefRef>,
        resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    ) -> Option<()> {
        let data = self.impls.get_mut(id)?;
        data.resolved_self_ty = resolved_self_ty;
        data.resolved_trait_ref = resolved_trait_ref;
        Some(())
    }

    pub fn statics(&self) -> &Arena<StaticId, StaticData> {
        &self.statics
    }

    /// Returns the semantic item lowered from one DefMap local definition.
    pub fn item_for_local_def(&self, local_def: LocalDefId) -> Option<ItemId> {
        self.local_items.get(local_def).copied().flatten()
    }

    pub fn lang_items(&self) -> &LangItemIndex {
        &self.lang_items
    }

    pub fn traits_with_refs(&self) -> impl Iterator<Item = (TraitDefRef, &TraitData)> {
        self.traits
            .iter_with_ids()
            .map(move |(id, data)| (TraitDefRef::new(self.origin, id), data))
    }

    pub fn impls_with_refs(&self) -> impl Iterator<Item = (ImplRef, &ImplData)> {
        self.impls
            .iter_with_ids()
            .map(move |(id, data)| (ImplRef::new(self.origin, id), data))
    }

    pub fn functions_with_refs(&self) -> impl Iterator<Item = (FunctionRef, &FunctionData)> {
        self.semantic_items().filter_map(|item| {
            let SemanticItemRef::Function(reference) = item.item() else {
                return None;
            };
            Some((reference, item.function_data()?))
        })
    }

    pub fn consts_with_refs(&self) -> impl Iterator<Item = (ConstRef, &ConstData)> {
        self.semantic_items().filter_map(|item| {
            let SemanticItemRef::Const(reference) = item.item() else {
                return None;
            };
            Some((reference, item.const_data()?))
        })
    }

    pub fn struct_data(&self, id: StructId) -> Option<&StructData> {
        self.structs.get(id)
    }

    pub fn union_data(&self, id: UnionId) -> Option<&UnionData> {
        self.unions.get(id)
    }

    pub fn enum_data(&self, id: EnumId) -> Option<&EnumData> {
        self.enums.get(id)
    }

    pub fn trait_data(&self, id: TraitId) -> Option<&TraitData> {
        self.traits.get(id)
    }

    pub fn impl_data(&self, id: ImplId) -> Option<&ImplData> {
        self.impls.get(id)
    }

    pub fn static_data(&self, id: StaticId) -> Option<&StaticData> {
        self.statics.get(id)
    }

    pub fn associated_items(&self, store: ItemStoreRef) -> Option<&AssociatedItemStore> {
        debug_assert_eq!(store.origin, self.origin, "Wrong lexical scope");
        self.members.get(&store.id)
    }

    pub fn semantic_item_view(&self, item: SemanticItemRef) -> Option<SemanticItemView<'_>> {
        let store = item.store();
        if store.id != ItemStoreId::Scope {
            return self.associated_items(store)?.item(item);
        }
        debug_assert_eq!(item.origin(), self.origin, "Wrong item store");

        // This is the semantic item boundary: callers can ask item-shaped questions without
        // spreading the arena-family match into higher layers.
        let data = match item {
            SemanticItemRef::TypeDef(ty) => match ty.id {
                TypeDefId::Struct(id) => SemanticItemData::Struct(self.struct_data(id)?),
                TypeDefId::Union(id) => SemanticItemData::Union(self.union_data(id)?),
                TypeDefId::Enum(id) => SemanticItemData::Enum(self.enum_data(id)?),
            },
            SemanticItemRef::Trait(trait_ref) => {
                SemanticItemData::Trait(self.trait_data(trait_ref.id)?)
            }
            SemanticItemRef::Impl(impl_ref) => SemanticItemData::Impl(self.impl_data(impl_ref.id)?),
            SemanticItemRef::Function(function_ref) => {
                SemanticItemData::Function(self.functions.get(function_ref.id)?)
            }
            SemanticItemRef::TypeAlias(type_alias_ref) => {
                SemanticItemData::TypeAlias(self.type_aliases.get(type_alias_ref.id)?)
            }
            SemanticItemRef::Const(const_ref) => {
                SemanticItemData::Const(self.consts.get(const_ref.id)?)
            }
            SemanticItemRef::Static(static_ref) => {
                SemanticItemData::Static(self.static_data(static_ref.id)?)
            }
        };

        Some(SemanticItemView::new(item, data))
    }

    pub fn semantic_items(&self) -> impl Iterator<Item = SemanticItemView<'_>> {
        // Follow header order rather than hash-map order for stable broad enumeration.
        let stores = self
            .traits_with_refs()
            .map(|(owner, _)| owner.member_store())
            .chain(
                self.impls_with_refs()
                    .map(|(owner, _)| owner.member_store()),
            );
        self.scope_items().chain(stores.flat_map(|store| {
            self.associated_items(store)
                .into_iter()
                .flat_map(move |members| members.semantic_items(store))
        }))
    }

    fn scope_items(&self) -> impl Iterator<Item = SemanticItemView<'_>> {
        let origin = self.origin;
        self.structs
            .iter_with_ids()
            .map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef::new_struct(origin, id).into(),
                    SemanticItemData::Struct(data),
                )
            })
            .chain(self.unions.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef::new_union(origin, id).into(),
                    SemanticItemData::Union(data),
                )
            }))
            .chain(self.enums.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef::new_enum(origin, id).into(),
                    SemanticItemData::Enum(data),
                )
            }))
            .chain(self.traits.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TraitDefRef { origin, id }.into(),
                    SemanticItemData::Trait(data),
                )
            }))
            .chain(self.impls.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(ImplRef { origin, id }.into(), SemanticItemData::Impl(data))
            }))
            .chain(self.functions.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    FunctionRef::new(ItemStoreRef::scope(origin), id).into(),
                    SemanticItemData::Function(data),
                )
            }))
            .chain(self.type_aliases.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeAliasRef::new(ItemStoreRef::scope(origin), id).into(),
                    SemanticItemData::TypeAlias(data),
                )
            }))
            .chain(self.consts.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    ConstRef::new(ItemStoreRef::scope(origin), id).into(),
                    SemanticItemData::Const(data),
                )
            }))
            .chain(self.statics.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    StaticRef::new(origin, id).into(),
                    SemanticItemData::Static(data),
                )
            }))
    }
}
