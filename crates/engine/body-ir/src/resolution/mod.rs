//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod autoderef;
mod body;
mod def_map_query;
mod deref;
mod impl_match;
mod index;
mod item_query;
mod method;
mod normalize;
mod pat;
mod ty;
mod type_path;

pub(crate) use self::{
    body::BodyResolver,
    body::BodyValuePathResolver,
    def_map_query::BodyDefMapSource,
    impl_match::BodyImplMatcher,
    index::SemanticResolutionIndex,
    item_query::BodyItemStoreSource,
    method::{function_applies_to_receiver, trait_function_candidates_for_receiver},
    ty::ty_from_type_ref_in_context,
    type_path::BodyTypePathResolver,
};

pub use self::autoderef::{
    BodyAutoderef, BodyAutoderefCandidate, BodyAutoderefCandidates, BodyAutoderefMode,
    BodyReferencePeelingCandidates,
};

// TODO: Should not be here
pub(super) fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    // Resolution often merges candidates from local, inherent, and trait sources. Keeping order
    // while deduplicating makes snapshots stable without pretending this is a ranking policy.
    if !items.contains(&item) {
        items.push(item);
    }
}
