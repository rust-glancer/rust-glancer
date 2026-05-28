use rg_arena::Arena;
use rg_ir_model::{
    ConstId, EnumId, FunctionId, ImplId, ItemId, LocalDefId, StaticId, StructId, TraitId,
    TypeAliasId, UnionId,
};

use crate::{
    ConstData, EnumData, FunctionData, ImplData, StaticData, StructData, TraitData, TypeAliasData,
    UnionData,
};

/// Target-local storage for semantic items.
///
/// Semantic ids are dense indexes into these vectors. Keeping all item families in one store lets
/// lowering allocate ids cheaply while the public query surface exposes stable typed references.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct ItemStore {
    // Mapping from local def ID to semantic item ID.
    pub(crate) local_items: Arena<LocalDefId, Option<ItemId>>,

    pub(crate) structs: Arena<StructId, StructData>,
    pub(crate) unions: Arena<UnionId, UnionData>,
    pub(crate) enums: Arena<EnumId, EnumData>,
    pub(crate) traits: Arena<TraitId, TraitData>,
    pub(crate) impls: Arena<ImplId, ImplData>,
    pub(crate) functions: Arena<FunctionId, FunctionData>,
    pub(crate) type_aliases: Arena<TypeAliasId, TypeAliasData>,
    pub(crate) consts: Arena<ConstId, ConstData>,
    pub(crate) statics: Arena<StaticId, StaticData>,
}

impl ItemStore {
    pub(crate) fn new(local_def_count: usize) -> Self {
        Self {
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

    /// Returns the semantic item lowered from one DefMap local definition.
    pub fn item_for_local_def(&self, local_def: LocalDefId) -> Option<ItemId> {
        self.local_items.get(local_def).copied().flatten()
    }

    pub(crate) fn set_local_item(&mut self, local_def: LocalDefId, item: ItemId) {
        let slot = self
            .local_items
            .get_mut(local_def)
            .expect("local item slot should exist while building semantic IR");
        *slot = Some(item);
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

    pub(crate) fn shrink_to_fit(&mut self) {
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

    pub(crate) fn alloc_struct(&mut self, data: StructData) -> StructId {
        self.structs.alloc(data)
    }

    pub(crate) fn alloc_union(&mut self, data: UnionData) -> UnionId {
        self.unions.alloc(data)
    }

    pub(crate) fn alloc_enum(&mut self, data: EnumData) -> EnumId {
        self.enums.alloc(data)
    }

    pub(crate) fn alloc_trait(&mut self, data: TraitData) -> TraitId {
        self.traits.alloc(data)
    }

    pub(crate) fn alloc_impl(&mut self, data: ImplData) -> ImplId {
        self.impls.alloc(data)
    }

    pub(crate) fn alloc_function(&mut self, data: FunctionData) -> FunctionId {
        self.functions.alloc(data)
    }

    pub(crate) fn alloc_type_alias(&mut self, data: TypeAliasData) -> TypeAliasId {
        self.type_aliases.alloc(data)
    }

    pub(crate) fn alloc_const(&mut self, data: ConstData) -> ConstId {
        self.consts.alloc(data)
    }

    pub(crate) fn alloc_static(&mut self, data: StaticData) -> StaticId {
        self.statics.alloc(data)
    }
}
