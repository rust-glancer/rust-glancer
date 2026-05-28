//! Semantic IR domain model.

pub(crate) mod items;
pub(crate) mod package;
pub(crate) mod resolution;
pub(crate) mod signature;
pub(crate) mod stats;

pub use self::{
    items::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        StructData, TraitData, TypeAliasData, UnionData,
    },
    package::PackageIr,
    resolution::{SemanticTypePathResolution, TypePathContext},
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    stats::SemanticIrStats,
};
