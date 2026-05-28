use rg_arena::Arena;
use rg_ir_model::{
    ConstId, ConstRef, EnumId, FunctionId, FunctionRef, ImplId, ImplRef, ItemId, LocalDefId,
    StaticId, StaticRef, StructId, TargetRef, TraitId, TraitRef, TypeAliasId, TypeAliasRef,
    TypeDefId, TypeDefRef, UnionId,
};

use crate::{
    ConstData, EnumData, FunctionData, ImplData, SemanticItemView, StaticData, StructData,
    TraitData, TypeAliasData, UnionData, view::SemanticItemData,
};

/// Target-local storage for semantic items.
///
/// Semantic ids are dense indexes into these vectors. Keeping all item families in one store lets
/// lowering allocate ids cheaply while the public query surface exposes stable typed references.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct ItemStore {
    // Target this item store corresponds to
    target_ref: TargetRef,

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
    pub(crate) fn new(target_ref: TargetRef, local_def_count: usize) -> Self {
        Self {
            target_ref,
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

    pub fn target_ref(&self) -> TargetRef {
        self.target_ref
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

    pub fn traits_with_refs(&self) -> impl Iterator<Item = (TraitRef, &TraitData)> {
        self.traits.iter_with_ids().map(move |(id, data)| {
            (
                TraitRef {
                    target: self.target_ref,
                    id,
                },
                data,
            )
        })
    }

    pub fn impls_with_refs(&self) -> impl Iterator<Item = (ImplRef, &ImplData)> {
        self.impls.iter_with_ids().map(move |(id, data)| {
            (
                ImplRef {
                    target: self.target_ref,
                    id,
                },
                data,
            )
        })
    }

    pub fn functions_with_refs(&self) -> impl Iterator<Item = (FunctionRef, &FunctionData)> {
        self.functions.iter_with_ids().map(move |(id, data)| {
            (
                FunctionRef {
                    target: self.target_ref,
                    id,
                },
                data,
            )
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

    pub fn semantic_items(&self) -> impl Iterator<Item = SemanticItemView<'_>> {
        let target = self.target_ref;
        // TODO: data should contain necessary refs inside
        self.structs
            .iter_with_ids()
            .map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef {
                        target,
                        id: TypeDefId::Struct(id),
                    }
                    .into(),
                    SemanticItemData::Struct(data),
                )
            })
            .chain(self.unions.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef {
                        target,
                        id: TypeDefId::Union(id),
                    }
                    .into(),
                    SemanticItemData::Union(data),
                )
            }))
            .chain(self.enums.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeDefRef {
                        target,
                        id: TypeDefId::Enum(id),
                    }
                    .into(),
                    SemanticItemData::Enum(data),
                )
            }))
            .chain(self.traits.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TraitRef { target, id }.into(),
                    SemanticItemData::Trait(data),
                )
            }))
            .chain(self.impls.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(ImplRef { target, id }.into(), SemanticItemData::Impl(data))
            }))
            .chain(self.functions.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    FunctionRef { target, id }.into(),
                    SemanticItemData::Function(data),
                )
            }))
            .chain(self.type_aliases.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    TypeAliasRef { target, id }.into(),
                    SemanticItemData::TypeAlias(data),
                )
            }))
            .chain(self.consts.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    ConstRef { target, id }.into(),
                    SemanticItemData::Const(data),
                )
            }))
            .chain(self.statics.iter_with_ids().map(move |(id, data)| {
                SemanticItemView::new(
                    StaticRef { target, id }.into(),
                    SemanticItemData::Static(data),
                )
            }))
    }
}
