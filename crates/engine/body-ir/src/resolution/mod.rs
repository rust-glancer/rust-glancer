//! Semantic resolution and type inference for Body IR.
//!
//! The resolver borrows immutable body structure, combines lexical/item lookup with canonical
//! signatures and trait obligations, and finalizes the resulting types and sparse selections into
//! the persisted `BodyFacts` sidecar.

mod infer;
mod pass;
mod query;
mod source;

pub(crate) use self::pass::BodyResolutionPass;

pub use self::{
    query::{BodyMethodQuery, BodyTypePathQuery, BodyValuePathQuery},
    source::BodyResolutionContext,
};
