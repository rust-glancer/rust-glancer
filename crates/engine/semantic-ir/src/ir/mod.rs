//! Semantic IR domain model.

pub(crate) mod ids;
pub(crate) mod items;
pub(crate) mod package;
pub(crate) mod resolution;
pub(crate) mod signature;
pub(crate) mod stats;
pub(crate) mod target;

pub use self::{
    ids::{
        AssocItemId, ConstRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, ItemOwner,
        SemanticItemKind, SemanticItemRef, StaticRef, TraitImplRef, TraitRef, TypeAliasRef,
        TypeDefId, TypeDefRef,
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
};
