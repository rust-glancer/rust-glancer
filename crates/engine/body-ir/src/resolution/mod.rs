//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod body;
mod normalize;
mod pat;
mod query_source;
mod type_path;

pub(crate) use self::{
    body::BodyResolver, body::BodyValuePathResolver, query_source::BodyQuerySource,
    type_path::BodyTypePathResolver,
};

// TODO: Should not be here
pub(super) fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    // Resolution often merges candidates from local, inherent, and trait sources. Keeping order
    // while deduplicating makes snapshots stable without pretending this is a ranking policy.
    if !items.contains(&item) {
        items.push(item);
    }
}
