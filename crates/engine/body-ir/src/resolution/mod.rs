//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod infer;
mod pass;
mod query;
mod source;
mod support;

pub(crate) use self::{
    pass::BodyResolutionPass,
    query::{CallSite, MethodCallSite, TypeRefUseSite},
    source::{BodyQuerySource, BodyResolutionProviders},
};

pub use self::{
    query::{BodyMethodQuery, BodyTypePathQuery, BodyValuePathQuery},
    source::BodyResolutionContext,
};
