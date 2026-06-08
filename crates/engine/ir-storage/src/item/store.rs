use rg_arena::Arena;
use rg_ir_model::{
    ConstId, ConstRef, DefMapRef, EnumId, FunctionId, FunctionRef, ImplId, ImplRef, ItemId,
    LocalDefId, SemanticItemRef, StaticId, StaticRef, StructId, TargetRef, TraitId, TraitRef,
    TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
    hir::items::{
        ConstData, EnumData, FunctionData, ImplData, StaticData, StructData, TraitData,
        TypeAliasData, UnionData,
    },
};
use rg_std::{MemorySize, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

use super::{SemanticItemView, view::SemanticItemData};

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
        }
    }

    pub fn set_local_item(&mut self, local_def: LocalDefId, item: ItemId) {
        let slot = self
            .local_items
            .get_mut(local_def)
            .expect("local item slot should exist while building semantic IR");
        *slot = Some(item);
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
        }
    }
}

/// Target-local storage for semantic items.
///
/// Semantic ids are dense indexes into these vectors. Keeping all item families in one store lets
/// lowering allocate ids cheaply while the public query surface exposes stable typed references.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
}

impl ItemStore {
    pub fn origin(&self) -> DefMapRef {
        self.origin
    }

    pub fn target_ref(&self) -> TargetRef {
        self.origin.origin_target()
    }

    pub fn structs(&self) -> &Arena<StructId, StructData> {
        &self.structs
    }

    pub fn unions(&self) -> &Arena<UnionId, UnionData> {
        &self.unions
    }

    pub fn enums(&self) -> &Arena<EnumId, EnumData> {
        &self.enums
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
        resolved_self_tys: UniqueVec<TypeDefRef>,
        resolved_trait_refs: UniqueVec<TraitRef>,
    ) -> Option<()> {
        let data = self.impls.get_mut(id)?;
        data.resolved_self_tys = resolved_self_tys;
        data.resolved_trait_refs = resolved_trait_refs;
        Some(())
    }

    pub fn functions(&self) -> &Arena<FunctionId, FunctionData> {
        &self.functions
    }

    pub fn type_aliases(&self) -> &Arena<TypeAliasId, TypeAliasData> {
        &self.type_aliases
    }

    pub fn consts(&self) -> &Arena<ConstId, ConstData> {
        &self.consts
    }

    pub fn statics(&self) -> &Arena<StaticId, StaticData> {
        &self.statics
    }

    /// Returns the semantic item lowered from one DefMap local definition.
    pub fn item_for_local_def(&self, local_def: LocalDefId) -> Option<ItemId> {
        self.local_items.get(local_def).copied().flatten()
    }

    pub fn traits_with_refs(&self) -> impl Iterator<Item = (TraitRef, &TraitData)> {
        self.traits
            .iter_with_ids()
            .map(move |(id, data)| (TraitRef::new(self.origin, id), data))
    }

    pub fn impls_with_refs(&self) -> impl Iterator<Item = (ImplRef, &ImplData)> {
        self.impls
            .iter_with_ids()
            .map(move |(id, data)| (ImplRef::new(self.origin, id), data))
    }

    pub fn functions_with_refs(&self) -> impl Iterator<Item = (FunctionRef, &FunctionData)> {
        self.functions
            .iter_with_ids()
            .map(move |(id, data)| (FunctionRef::new(self.origin, id), data))
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

    pub fn function_data(&self, id: FunctionId) -> Option<&FunctionData> {
        self.functions.get(id)
    }

    pub fn type_alias_data(&self, id: TypeAliasId) -> Option<&TypeAliasData> {
        self.type_aliases.get(id)
    }

    pub fn const_data(&self, id: ConstId) -> Option<&ConstData> {
        self.consts.get(id)
    }

    pub fn static_data(&self, id: StaticId) -> Option<&StaticData> {
        self.statics.get(id)
    }

    pub fn semantic_item_view(&self, item: SemanticItemRef) -> Option<SemanticItemView<'_>> {
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
                SemanticItemData::Function(self.function_data(function_ref.id)?)
            }
            SemanticItemRef::TypeAlias(type_alias_ref) => {
                SemanticItemData::TypeAlias(self.type_alias_data(type_alias_ref.id)?)
            }
            SemanticItemRef::Const(const_ref) => {
                SemanticItemData::Const(self.const_data(const_ref.id)?)
            }
            SemanticItemRef::Static(static_ref) => {
                SemanticItemData::Static(self.static_data(static_ref.id)?)
            }
        };

        Some(SemanticItemView::new(item, data))
    }

    pub fn shrink_to_fit(&mut self) {
        self.local_items.shrink_to_fit();
        self.structs.shrink_to_fit();
        for data in self.structs.iter_mut() {
            data.shrink_to_fit();
        }
        self.unions.shrink_to_fit();
        for data in self.unions.iter_mut() {
            data.shrink_to_fit();
        }
        self.enums.shrink_to_fit();
        for data in self.enums.iter_mut() {
            data.shrink_to_fit();
        }
        self.traits.shrink_to_fit();
        for data in self.traits.iter_mut() {
            data.shrink_to_fit();
        }
        self.impls.shrink_to_fit();
        for data in self.impls.iter_mut() {
            data.shrink_to_fit();
        }
        self.functions.shrink_to_fit();
        for data in self.functions.iter_mut() {
            data.shrink_to_fit();
        }
        self.type_aliases.shrink_to_fit();
        for data in self.type_aliases.iter_mut() {
            data.shrink_to_fit();
        }
        self.consts.shrink_to_fit();
        for data in self.consts.iter_mut() {
            data.shrink_to_fit();
        }
        self.statics.shrink_to_fit();
        for data in self.statics.iter_mut() {
            data.shrink_to_fit();
        }
    }

    pub fn semantic_items(&self) -> impl Iterator<Item = SemanticItemView<'_>> {
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
                    TraitRef { origin, id }.into(),
                    SemanticItemData::Trait(data),
                )
            }))
            .chain(self.impls.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(ImplRef { origin, id }.into(), SemanticItemData::Impl(data))
            }))
            .chain(self.functions.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    FunctionRef { origin, id }.into(),
                    SemanticItemData::Function(data),
                )
            }))
            .chain(self.type_aliases.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeAliasRef { origin, id }.into(),
                    SemanticItemData::TypeAlias(data),
                )
            }))
            .chain(self.consts.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    ConstRef { origin, id }.into(),
                    SemanticItemData::Const(data),
                )
            }))
            .chain(self.statics.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    StaticRef { origin, id }.into(),
                    SemanticItemData::Static(data),
                )
            }))
    }

    pub fn type_def_name(&self, id: TypeDefId) -> Option<&str> {
        match id {
            TypeDefId::Struct(id) => self.struct_data(id).map(|data| data.name.as_str()),
            TypeDefId::Enum(id) => self.enum_data(id).map(|data| data.name.as_str()),
            TypeDefId::Union(id) => self.union_data(id).map(|data| data.name.as_str()),
        }
    }

    pub fn generic_params_for_type_def(
        &self,
        id: TypeDefId,
    ) -> Option<&rg_ir_model::items::GenericParams> {
        match id {
            TypeDefId::Struct(id) => self.struct_data(id).map(|data| &data.generics),
            TypeDefId::Enum(id) => self.enum_data(id).map(|data| &data.generics),
            TypeDefId::Union(id) => self.union_data(id).map(|data| &data.generics),
        }
    }
}
