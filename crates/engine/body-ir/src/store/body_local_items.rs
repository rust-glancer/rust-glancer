//! Definition and semantic-item storage declared inside one body.

use std::collections::HashMap;

use rg_def_map::DefMap;
use rg_ir_model::{
    ConstRef, FunctionRef, ImplId, ItemStoreId, ItemStoreRef, SemanticItemRef, TraitDefRef,
    TypeDefRef,
};
use rg_semantic_ir::{AssociatedItemStore, ConstData, FunctionData, ItemStore, SemanticItemView};
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Definitions and declarations inside one body. The scope store holds free declarations and
/// trait/impl headers; their members live in separate stores. Body collection fills both before
/// publishing this result, so local readers never need to discover more members.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyLocalItems {
    def_map: DefMap,
    item_store: ItemStore,
    members: HashMap<ItemStoreId, AssociatedItemStore>,
}

impl BodyLocalItems {
    pub(crate) fn new(
        def_map: DefMap,
        item_store: ItemStore,
        members: HashMap<ItemStoreId, AssociatedItemStore>,
    ) -> Self {
        Self {
            def_map,
            item_store,
            members,
        }
    }

    pub fn def_map(&self) -> &DefMap {
        &self.def_map
    }

    pub fn item_store(&self) -> &ItemStore {
        &self.item_store
    }

    pub fn associated_items(&self, store: ItemStoreRef) -> Option<&AssociatedItemStore> {
        if store.origin != self.item_store.origin() {
            return None;
        }
        self.members.get(&store.id)
    }

    pub fn item(&self, item: SemanticItemRef) -> Option<SemanticItemView<'_>> {
        if item.store().id == ItemStoreId::Scope {
            self.item_store.semantic_item_view(item)
        } else {
            self.associated_items(item.store())?.item(item)
        }
    }

    /// All declarations are ready after body collection. Header order keeps enumeration stable.
    pub fn all_items(&self) -> impl Iterator<Item = SemanticItemView<'_>> {
        self.item_store
            .scope_items()
            .chain(self.item_store.member_stores().flat_map(|store| {
                self.members
                    .get(&store.id)
                    .into_iter()
                    .flat_map(move |members| members.semantic_items(store))
            }))
    }

    pub(crate) fn functions_with_refs(&self) -> impl Iterator<Item = (FunctionRef, &FunctionData)> {
        self.all_items().filter_map(|item| {
            let SemanticItemRef::Function(reference) = item.item() else {
                return None;
            };
            Some((reference, item.function_data()?))
        })
    }

    pub(crate) fn consts_with_refs(&self) -> impl Iterator<Item = (ConstRef, &ConstData)> {
        self.all_items().filter_map(|item| {
            let SemanticItemRef::Const(reference) = item.item() else {
                return None;
            };
            Some((reference, item.const_data()?))
        })
    }

    pub fn set_impl_header_facts(
        &mut self,
        id: ImplId,
        resolved_self_ty: ExpectedUnique<TypeDefRef>,
        resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    ) -> Option<()> {
        self.item_store
            .set_impl_header_facts(id, resolved_self_ty, resolved_trait_ref)
    }
}
