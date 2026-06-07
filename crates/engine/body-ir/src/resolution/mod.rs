//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod pass;
mod query;
mod source;
mod support;

pub(crate) use self::{
    pass::BodyResolver,
    query::{BodyTypePathResolver, BodyValuePathResolver, TypeRefUseSite},
    source::{BodyQuerySource, BodyResolutionContext, BodyResolutionProviders},
    support::push_unique,
};

pub use self::query::BodyScopeQuery;
