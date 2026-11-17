//! Semantic item data, storage, and definition-only queries.
//!
//! These modules form one ownership boundary: item data and compact signatures are stored here,
//! local DefMap identities are indexed here, and path resolution stops at semantic item refs here.
//! Callers that need a `Ty` can project the resulting refs in the type engine without pulling type
//! machinery into item storage or definition-only queries.

mod context;
mod data;
mod generics;
mod lang_item;
mod lookup_env;
mod lookup_index;
mod lookup_keys;
mod lookup_shard;
mod query;
mod signature;
mod store;
mod type_path_resolution;
mod view;

pub use self::{
    context::{SelfTypeOwner, TypePathContext},
    data::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        StructData, TraitData, TypeAliasData, UnionData,
    },
    generics::{GenericParamSource, GenericParamView, Generics},
    lang_item::LangItemIndex,
    lookup_env::ItemLookupEnv,
    lookup_index::{ItemLookupIndex, TraitImplSelfHead},
    lookup_keys::LookupKeys,
    lookup_shard::ItemLookupShard,
    query::{
        GenericsQuery, ItemLookupQuery, ItemResolutionQuery, ItemStoreQuery, LookupAnswer,
        LookupQuestion, SemanticItemSource,
    },
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    store::{ItemStore, ItemStoreBuilder},
    type_path_resolution::TypePathResolution,
    view::SemanticItemView,
};
