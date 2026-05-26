mod build;
mod cursor;
mod ir;
mod store;
mod view;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{
        ConstData, ConstSignature, EnumData, EnumVariantData, FieldData, FunctionData,
        FunctionSignature, ImplData, ItemStore, PackageIr, SemanticIrStats,
        SemanticTypePathResolution, StaticData, StructData, TargetIr, TraitData, TypeAliasData,
        TypeAliasSignature, TypePathContext, UnionData,
    },
    store::{SemanticIrDb, SemanticIrReadTxn},
    view::SemanticItemView,
};

pub(crate) use self::ir::{
    AssocItemId, ConstRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, ItemOwner,
    SemanticItemKind, SemanticItemRef, StaticRef, TraitImplRef, TraitRef, TypeAliasRef, TypeDefId,
    TypeDefRef,
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
