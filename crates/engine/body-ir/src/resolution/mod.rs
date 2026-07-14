//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod infer;
mod pass;
mod query;
mod source;

pub(crate) use self::{
    pass::BodyResolutionPass,
    query::{CallSite, MethodCallSite},
    source::BodyQuerySource,
};

pub use self::{
    query::{BodyMethodQuery, BodyTypePathQuery, BodyValuePathQuery},
    source::BodyResolutionContext,
};
