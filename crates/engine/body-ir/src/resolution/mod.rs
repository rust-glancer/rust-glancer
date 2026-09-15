//! Semantic resolution and type inference for Body IR.
//!
//! The resolver borrows immutable body structure, combines lexical/item lookup with canonical
//! signatures and trait obligations, and finalizes the resulting types and sparse selections into
//! the persisted `BodyFacts` sidecar.
//!
//! `pass` drives the fixed point, `infer` maintains live type evidence, and `query` answers the
//! semantic questions each step asks. The context and source adapter keep query inputs coherent;
//! the body-owned caches survive as the pass creates new views of its evolving facts.

mod cache;
mod context;
mod infer;
mod pass;
mod query;
mod source;

pub(crate) use self::{cache::BodyResolutionCaches, pass::BodyResolutionPass};

pub use self::{
    context::BodyResolutionContext,
    query::{BodyMethodQuery, BodyTypePathQuery, BodyValuePathQuery},
};
