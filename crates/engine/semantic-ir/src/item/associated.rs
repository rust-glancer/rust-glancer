use rg_arena::Arena;
use rg_ir_model::{AssocItemId, ConstId, FunctionId, ItemStoreRef, SemanticItemRef, TypeAliasId};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{ConstData, FunctionData, SemanticItemView, TypeAliasData, view::SemanticItemData};

/// Functions, associated types and constants declared inside one trait or impl block.
///
/// Each block owns its own store, even when two impls have the same receiver type. Its header
/// stays in the enclosing [`super::ItemStore`] and supplies inherited generics and `Self`.
/// Local IDs start afresh for each owner, so adding members to one impl cannot renumber another
/// impl's members. Outside this store, pair each local ID with its [`ItemStoreRef`].
///
/// These payloads describe declarations; their bodies and initializers are handled by Body IR.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct AssociatedItemStore {
    items: Vec<AssocItemId>,
    functions: Arena<FunctionId, FunctionData>,
    type_aliases: Arena<TypeAliasId, TypeAliasData>,
    consts: Arena<ConstId, ConstData>,
}

impl AssociatedItemStore {
    pub fn items(&self) -> &[AssocItemId] {
        &self.items
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

    pub(crate) fn item(&self, item: SemanticItemRef) -> Option<SemanticItemView<'_>> {
        let data = match item {
            SemanticItemRef::Function(item) => {
                SemanticItemData::Function(self.function_data(item.id)?)
            }
            SemanticItemRef::TypeAlias(item) => {
                SemanticItemData::TypeAlias(self.type_alias_data(item.id)?)
            }
            SemanticItemRef::Const(item) => SemanticItemData::Const(self.const_data(item.id)?),
            _ => return None,
        };
        Some(SemanticItemView::new(item, data))
    }

    pub(crate) fn semantic_items(
        &self,
        store: ItemStoreRef,
    ) -> impl Iterator<Item = SemanticItemView<'_>> {
        self.items
            .iter()
            .filter_map(move |id| self.item(id.semantic_ref(store)))
    }
}

/// Allocates members and their source-order list together, so an arena entry cannot be omitted
/// from enumeration. Each builder belongs to one owner.
#[derive(Debug, Default)]
pub struct AssociatedItemStoreBuilder {
    store: AssociatedItemStore,
}

impl AssociatedItemStoreBuilder {
    pub fn alloc_function(&mut self, data: FunctionData) -> FunctionId {
        let id = self.store.functions.alloc(data);
        self.store.items.push(AssocItemId::Function(id));
        id
    }

    pub fn alloc_type_alias(&mut self, data: TypeAliasData) -> TypeAliasId {
        let id = self.store.type_aliases.alloc(data);
        self.store.items.push(AssocItemId::TypeAlias(id));
        id
    }

    pub fn alloc_const(&mut self, data: ConstData) -> ConstId {
        let id = self.store.consts.alloc(data);
        self.store.items.push(AssocItemId::Const(id));
        id
    }

    pub fn build(self) -> AssociatedItemStore {
        self.store
    }
}
