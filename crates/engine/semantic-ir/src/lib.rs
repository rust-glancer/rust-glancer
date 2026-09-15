//! Definition-level semantic IR and its query-facing storage.
//!
//! DefMap establishes names, namespaces, visibility, and stable local-definition identities.
//! This crate lowers those definitions into language items such as structs, functions, traits,
//! impls, and signatures, then exposes crate-scoped stores and resolution queries over them.
//!
//! Types remain syntax-shaped here. Projecting a [`rg_item_tree::TypeRef`] into the type
//! engine belongs to `rg_ty`, which depends on this crate rather than participating in definition
//! ownership.

mod build;
mod item;
mod package;
mod stats;
mod store;
#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use self::{
    build::{ItemStoreLowerer, ItemStoreSourceReader},
    item::{
        ConstData, ConstSignature, CrateItemQuery, EnumData, EnumVariantData, FieldData,
        FunctionData, FunctionSignature, GenericParamSource, GenericParamView, Generics,
        GenericsQuery, ImplData, ItemLookupIndex, ItemLookupIndexSource, ItemLookupQuery,
        ItemLookupQueryCache, ItemLookupQueryCacheStats, ItemResolutionQuery, ItemStore,
        ItemStoreBuilder, ItemStoreQuery, ItemStoreSource, SelfTypeOwner, SemanticItemView,
        StaticData, StructData, TraitData, TraitImplSelfHead, TypeAliasData, TypeAliasSignature,
        TypePathContext, TypePathResolution, UnionData,
    },
    package::{SemanticCrate, SemanticPackage, SemanticPackageManifest},
    stats::SemanticIrStats,
    store::{LoadSemanticIr, SemanticIrDb, SemanticIrLoader, SemanticIrReadTxn},
};
