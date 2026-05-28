//! Semantic IR domain model.

pub(crate) mod package;
pub(crate) mod resolution;
pub(crate) mod stats;

// TODO: We should not hide the origin, to be removed eventually.
// Keeping it here for now to not spend time fixing imports literally everywhere.
pub use rg_ir_model::hir::{
    items::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        StructData, TraitData, TypeAliasData, UnionData,
    },
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
};

pub use self::{
    package::PackageIr,
    resolution::{SemanticTypePathResolution, TypePathContext},
    stats::SemanticIrStats,
};
