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
};

pub(crate) use self::signature::SignatureGenerics;
