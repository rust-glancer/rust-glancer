//! Definition-level semantic IR and its query-facing storage.
//!
//! DefMap establishes names, namespaces, visibility, and stable local-definition identities.
//! This crate lowers those definitions into language items such as structs, functions, traits,
//! impls, and signatures, then exposes crate-scoped stores and resolution queries over them.
//!
//! Types remain syntax-shaped here. Projecting a [`rg_ir_model::items::TypeRef`] into the type
//! engine belongs to `rg_ty`, which depends on this crate rather than participating in definition
//! ownership.

mod build;
mod cursor;
mod ir;
mod item;
mod store;
#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{PackageIr, SemanticIrStats},
    item::{
        ConstData, ConstSignature, CrateItemQuery, EnumData, EnumVariantData, FieldData,
        FunctionData, FunctionSignature, ImplData, ItemLookupIndex, ItemResolutionQuery, ItemStore,
        ItemStoreBuilder, ItemStoreLowerer, ItemStoreQuery, ItemStoreSource, ItemStoreSourceReader,
        SemanticItemView, StaticData, StructData, TraitData, TypeAliasData, TypeAliasSignature,
        TypePathContext, UnionData,
    },
    store::{SemanticIrDb, SemanticIrReadTxn},
};
