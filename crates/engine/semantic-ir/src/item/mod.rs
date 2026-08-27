//! Semantic item data, storage, and definition-only queries.
//!
//! These modules form one ownership boundary: item data and compact signatures are stored here,
//! local DefMap identities are indexed here, and path resolution stops at semantic item refs here.
//! Callers that need a `Ty` can project the resulting refs in the type engine without pulling type
//! machinery into definition lowering.

mod context;
mod data;
mod generics;
mod lang_item;
mod lookup_index;
mod lowering;
mod query;
mod signature;
mod store;
mod type_path_resolution;
mod view;

pub use self::{
    context::TypePathContext,
    data::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        StructData, TraitData, TypeAliasData, UnionData,
    },
    generics::{GenericParamSource, GenericParamView, Generics},
    lookup_index::{ItemLookupIndex, TraitImplSelfHead},
    lowering::{ItemStoreLowerer, ItemStoreSourceReader},
    query::{
        CrateItemQuery, GenericsQuery, ItemLookupIndexSource, ItemLookupQuery,
        ItemLookupQueryCache, ItemLookupQueryCacheStats, ItemResolutionQuery, ItemStoreQuery,
        ItemStoreSource,
    },
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    store::{ItemStore, ItemStoreBuilder},
    type_path_resolution::TypePathResolution,
    view::SemanticItemView,
};

pub(crate) use self::lookup_index::TraitItemTraitRefs;
