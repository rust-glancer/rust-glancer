//! Semantic resolution and type inference for Body IR.
//!
//! The resolver borrows immutable body structure, combines lexical/item lookup with canonical
//! signatures and trait obligations, and finalizes the resulting types and sparse selections into
//! the persisted `BodyFacts` sidecar.
//!
//! `infer` owns recursive traversal, live type evidence, and pending semantic work. `query` answers
//! its lookup questions. The context keeps query inputs coherent; body-owned
//! caches share declaration results across expressions and deferred lookups.

mod cache;
mod context;
mod infer;
mod query;

pub(crate) use self::infer::InferenceContext;

pub use self::{
    context::BodyResolutionContext,
    query::{BodyMethodQuery, BodyTypePathQuery, BodyValuePathQuery},
};
