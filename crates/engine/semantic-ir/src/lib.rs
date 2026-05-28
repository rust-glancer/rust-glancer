mod build;
mod cursor;
mod ir;
mod item_store;
mod store;
mod view;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{
        ConstData, ConstSignature, EnumData, EnumVariantData, FieldData, FunctionData,
        FunctionSignature, ImplData, PackageIr, SemanticIrStats, SemanticTypePathResolution,
        StaticData, StructData, TraitData, TypeAliasData, TypeAliasSignature, TypePathContext,
        UnionData,
    },
    item_store::ItemStore,
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
