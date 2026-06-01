mod build;
mod cursor;
mod ir;
mod item_store;
mod item_store_lowering;
mod store;
mod view;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{PackageIr, SemanticIrStats, TypePathContext},
    item_store::{ItemStore, ItemStoreBuilder},
    item_store_lowering::{ItemStoreLowerer, ItemStoreSourceReader},
    store::{SemanticIrDb, SemanticIrReadTxn},
    view::SemanticItemView,
};

pub use rg_item_tree::{
    Documentation, EnumVariantItem, FieldItem, FieldKey, FieldList, FunctionItem,
    FunctionQualifiers, GenericParams, Mutability, ParamItem, TypeBound, TypeRef, VisibilityLevel,
    WherePredicate,
};

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
