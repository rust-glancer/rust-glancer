mod build;
mod cursor;
mod ir;
mod store;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{
        AssocItemId, ConstData, ConstId, ConstRef, ConstSignature, EnumData, EnumId,
        EnumVariantData, EnumVariantRef, FieldData, FieldRef, FunctionData, FunctionId,
        FunctionRef, FunctionSignature, ImplData, ImplId, ImplRef, ItemId, ItemOwner, ItemStore,
        PackageIr, SemanticIrStats, SemanticTypePathResolution, StaticData, StaticId, StaticRef,
        StructData, StructId, TargetIr, TraitApplicability, TraitData, TraitId, TraitImplRef,
        TraitRef, TypeAliasData, TypeAliasId, TypeAliasRef, TypeAliasSignature, TypeDefId,
        TypeDefRef, TypePathContext, UnionData, UnionId,
    },
    store::{SemanticIrDb, SemanticIrPackageBundle, SemanticIrReadTxn},
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
