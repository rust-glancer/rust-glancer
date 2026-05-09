mod build;
mod cache;
mod cursor;
mod db;
mod ids;
mod items;
mod memsize;
mod package;
mod resolution;
mod signature;
mod stats;
mod target;
mod txn;

#[cfg(test)]
mod tests;

pub use self::{
    cache::SemanticIrPackageBundle,
    cursor::SemanticCursorCandidate,
    db::SemanticIrDb,
    ids::{
        AssocItemId, ConstId, ConstRef, EnumId, EnumVariantRef, FieldRef, FunctionId, FunctionRef,
        ImplId, ImplRef, ItemId, ItemOwner, StaticId, StaticRef, StructId, TraitApplicability,
        TraitId, TraitImplRef, TraitRef, TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
    },
    items::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, ItemStore,
        StaticData, StructData, TraitData, TypeAliasData, UnionData,
    },
    package::PackageIr,
    resolution::{SemanticTypePathResolution, TypePathContext},
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    stats::SemanticIrStats,
    target::TargetIr,
    txn::SemanticIrReadTxn,
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
