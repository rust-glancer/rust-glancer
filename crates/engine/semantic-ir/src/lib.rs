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
mod reader;
mod source;
#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use rg_source::ReadError;

pub(crate) use self::source::DeclarationSourceResolver;
pub use self::{
    build::{
        AssociatedDeclarations, BodyOwnerData, ItemStoreLowerer, ItemStoreSourceReader,
        SymbolNameData, SymbolNameIndex, TargetDeclarationMetadata, TargetDeclarationReader,
        TargetDeclarations, TargetItemData, TargetSourceFiles, associated_declarations, body_owner,
        body_owners, target_declarations,
    },
    item::{
        AssociatedItemFilter, AssociatedItemStore, AssociatedItemStoreBuilder, ConstData,
        ConstSignature, EnumData, EnumVariantData, FieldData, FunctionData, FunctionSignature,
        GenericParamSource, GenericParamView, Generics, GenericsQuery, ImplData, ItemLookupEnv,
        ItemLookupIndex, ItemLookupQuery, ItemLookupShard, ItemResolutionQuery, ItemStore,
        ItemStoreBuilder, ItemStoreQuery, LangItemIndex, LookupAnswer, LookupKeys, LookupQuestion,
        SelfTypeOwner, SemanticItemSource, SemanticItemView, StaticData, StructData, TraitData,
        TraitImplSelfHead, TypeAliasData, TypeAliasSignature, TypePathContext, TypePathResolution,
        UnionData,
    },
    reader::{CrateItemSource, ItemReader},
    source::{
        ItemSourceView, SourceOwnership, SourceOwnershipIndex, SourceSelection,
        complete_source_ownership,
    },
};
