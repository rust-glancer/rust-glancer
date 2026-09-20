//! Lookup results reused by semantic queries for one body.

mod body_items;
mod method;
mod traits;

pub(crate) use self::{
    body_items::{BodyLocalInherentItemNames, BodyLocalItemCache, BodyLocalItemIndex},
    method::BodyMethodCache,
    traits::{BodyTraitLookupCache, BodyTraitSurface},
};

/// Request-local semantic caches shared by queries for one body.
///
/// Inference retains one query context while it traverses the body and completes pending work.
/// Query objects cloned from that context share these handles. The declaration facts below stay
/// stable as types gain evidence:
///
/// - `traits` retains lexical trait sets and name-filtered declaration surfaces;
/// - `body_local_items` indexes active-overlay and body-local declarations once;
/// - `methods` remembers receiver/name combinations with no extension method.
///
/// Receiver-specific positive proofs stay in the inference-owned trait-selection scope instead.
/// A new body receives a new cache group, so none of these body identities escape their request.
#[derive(Clone, Default)]
pub(crate) struct BodyResolutionCaches {
    pub(crate) traits: BodyTraitLookupCache,
    pub(crate) body_local_items: BodyLocalItemCache,
    pub(crate) methods: BodyMethodCache,
}
