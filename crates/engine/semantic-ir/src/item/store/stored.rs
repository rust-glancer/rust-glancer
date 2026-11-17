//! Independently readable semantic declaration records, preserving the builder's typed IDs.

use std::collections::HashMap;

use rg_arena::{Arena, StoredArena};
use rg_ir_model::{
    ConstId, ConstRef, DefMapRef, EnumId, FileId, FunctionId, FunctionRef, ImplId, ImplRef, ItemId,
    ItemStoreId, ItemStoreRef, LocalDefId, SemanticItemRef, StaticId, StaticRef, StructId,
    TraitDefRef, TraitId, TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
};
use rg_source::QueryContext;
use wincode::{SchemaRead, SchemaWrite};

use super::ItemStore;
use crate::{
    ConstData, EnumData, FunctionData, ImplData, SemanticItemView, StaticData, StructData,
    TraitData, TypeAliasData, UnionData, item::view::SemanticItemData,
};

/// A target's lowered declarations, split into independently readable record groups.
///
/// Name resolution produces `LocalDefId`s; `local_items` maps those to the typed IDs used for
/// signature reads. The file index also contains only identities. Readers can therefore select
/// a declaration before loading its record group, which may include neighboring declarations.
/// Trait and impl records contain their headers; associated declarations live in separate stores.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct StoredItemStore {
    origin: DefMapRef,
    local_items: ajika::Output<Arena<LocalDefId, Option<ItemId>>>,
    files: ajika::Output<HashMap<FileId, Vec<SemanticItemRef>>>,
    structs: StoredArena<StructId, StructData>,
    unions: StoredArena<UnionId, UnionData>,
    enums: StoredArena<EnumId, EnumData>,
    traits: StoredArena<TraitId, TraitData>,
    impls: StoredArena<ImplId, ImplData>,
    functions: StoredArena<FunctionId, FunctionData>,
    type_aliases: StoredArena<TypeAliasId, TypeAliasData>,
    consts: StoredArena<ConstId, ConstData>,
    statics: StoredArena<StaticId, StaticData>,
}

impl StoredItemStore {
    pub(crate) fn publish(db: &QueryContext<'_>, store: ItemStore) -> ajika::Result<Self> {
        // Keep the exact refs allocated by lowering. Generated declarations and impl headers
        // must remain addressable through the same IDs after their records are split into groups.
        let mut files: HashMap<FileId, Vec<SemanticItemRef>> = HashMap::new();
        for item in store.scope_items() {
            files
                .entry(item.source().file_id)
                .or_default()
                .push(item.item());
        }
        Ok(Self {
            origin: store.origin,
            local_items: db.output("semantic_ids", store.local_items.into())?,
            files: db.output("semantic_files", files.into())?,
            structs: StoredArena::publish(db, "semantic_structs", store.structs)?,
            unions: StoredArena::publish(db, "semantic_unions", store.unions)?,
            enums: StoredArena::publish(db, "semantic_enums", store.enums)?,
            traits: StoredArena::publish(db, "semantic_traits", store.traits)?,
            impls: StoredArena::publish(db, "semantic_impls", store.impls)?,
            functions: StoredArena::publish(db, "semantic_functions", store.functions)?,
            type_aliases: StoredArena::publish(db, "semantic_type_aliases", store.type_aliases)?,
            consts: StoredArena::publish(db, "semantic_consts", store.consts)?,
            statics: StoredArena::publish(db, "semantic_statics", store.statics)?,
        })
    }

    pub(crate) fn item_for_local_def(
        &self,
        db: &QueryContext<'_>,
        id: LocalDefId,
    ) -> ajika::Result<Option<ItemId>> {
        Ok(self.local_items.value_ref(db)?.get(id).copied().flatten())
    }

    pub(crate) fn item<'db>(
        &self,
        db: &QueryContext<'db>,
        item: SemanticItemRef,
    ) -> ajika::Result<Option<SemanticItemView<'db>>> {
        if item.origin() != self.origin || item.store().id != ItemStoreId::Scope {
            return Ok(None);
        }
        let data = match item {
            SemanticItemRef::TypeDef(ty) => match ty.id {
                TypeDefId::Struct(id) => self.structs.get(db, id)?.map(SemanticItemData::Struct),
                TypeDefId::Union(id) => self.unions.get(db, id)?.map(SemanticItemData::Union),
                TypeDefId::Enum(id) => self.enums.get(db, id)?.map(SemanticItemData::Enum),
            },
            SemanticItemRef::Trait(item) => {
                self.traits.get(db, item.id)?.map(SemanticItemData::Trait)
            }
            SemanticItemRef::Impl(item) => self.impls.get(db, item.id)?.map(SemanticItemData::Impl),
            SemanticItemRef::Function(item) => self
                .functions
                .get(db, item.id)?
                .map(SemanticItemData::Function),
            SemanticItemRef::TypeAlias(item) => self
                .type_aliases
                .get(db, item.id)?
                .map(SemanticItemData::TypeAlias),
            SemanticItemRef::Const(item) => {
                self.consts.get(db, item.id)?.map(SemanticItemData::Const)
            }
            SemanticItemRef::Static(item) => {
                self.statics.get(db, item.id)?.map(SemanticItemData::Static)
            }
        };
        Ok(data.map(|data| SemanticItemView::new(item, data)))
    }

    /// Select identities by source file before acquiring their record groups. This includes
    /// generated declarations whose retained source points into that file.
    pub(crate) fn source_items<'db>(
        &self,
        db: &QueryContext<'db>,
        file: FileId,
    ) -> ajika::Result<Vec<SemanticItemView<'db>>> {
        self.files
            .value_ref(db)?
            .get(&file)
            .into_iter()
            .flatten()
            .copied()
            .map(|item| Ok(self.item(db, item)?.expect("indexed semantic item")))
            .collect()
    }

    /// Read every module-owned declaration and trait/impl header. Associated declarations remain
    /// in their own stores; file-local callers use `source_items` to select fewer record groups.
    pub(crate) fn scope_items<'db>(
        &self,
        db: &QueryContext<'db>,
    ) -> ajika::Result<Vec<SemanticItemView<'db>>> {
        let mut items = Vec::new();
        for (id, data) in self.structs.entries(db)? {
            items.push(SemanticItemView::new(
                TypeDefRef::new_struct(self.origin, id).into(),
                SemanticItemData::Struct(data),
            ));
        }
        for (id, data) in self.unions.entries(db)? {
            items.push(SemanticItemView::new(
                TypeDefRef::new_union(self.origin, id).into(),
                SemanticItemData::Union(data),
            ));
        }
        for (id, data) in self.enums.entries(db)? {
            items.push(SemanticItemView::new(
                TypeDefRef::new_enum(self.origin, id).into(),
                SemanticItemData::Enum(data),
            ));
        }
        for (id, data) in self.traits.entries(db)? {
            items.push(SemanticItemView::new(
                TraitDefRef::new(self.origin, id).into(),
                SemanticItemData::Trait(data),
            ));
        }
        for (id, data) in self.impls.entries(db)? {
            items.push(SemanticItemView::new(
                ImplRef::new(self.origin, id).into(),
                SemanticItemData::Impl(data),
            ));
        }
        for (id, data) in self.functions.entries(db)? {
            items.push(SemanticItemView::new(
                FunctionRef::new(ItemStoreRef::scope(self.origin), id).into(),
                SemanticItemData::Function(data),
            ));
        }
        for (id, data) in self.type_aliases.entries(db)? {
            items.push(SemanticItemView::new(
                TypeAliasRef::new(ItemStoreRef::scope(self.origin), id).into(),
                SemanticItemData::TypeAlias(data),
            ));
        }
        for (id, data) in self.consts.entries(db)? {
            items.push(SemanticItemView::new(
                ConstRef::new(ItemStoreRef::scope(self.origin), id).into(),
                SemanticItemData::Const(data),
            ));
        }
        for (id, data) in self.statics.entries(db)? {
            items.push(SemanticItemView::new(
                StaticRef::new(self.origin, id).into(),
                SemanticItemData::Static(data),
            ));
        }
        Ok(items)
    }

    /// List trait and impl owners from the allocated arena IDs without loading their headers.
    pub(crate) fn member_stores(&self) -> impl Iterator<Item = ItemStoreRef> + '_ {
        self.traits
            .ids()
            .map(|id| TraitDefRef::new(self.origin, id).member_store())
            .chain(
                self.impls
                    .ids()
                    .map(|id| ImplRef::new(self.origin, id).member_store()),
            )
    }

    pub(crate) fn trait_data<'db>(
        &self,
        db: &QueryContext<'db>,
        id: TraitId,
    ) -> ajika::Result<Option<&'db TraitData>> {
        self.traits.get(db, id)
    }

    pub(crate) fn impl_data<'db>(
        &self,
        db: &QueryContext<'db>,
        id: ImplId,
    ) -> ajika::Result<Option<&'db ImplData>> {
        self.impls.get(db, id)
    }
}
