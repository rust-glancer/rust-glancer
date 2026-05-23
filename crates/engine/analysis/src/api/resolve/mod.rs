//! Cross-IR resolution helpers shared by analysis queries.
//!
//! This layer normalizes cursor candidates and resolved identities from def-map, semantic IR, and
//! body IR into analysis-owned vocabularies before query modules turn them into navigation, hover,
//! type, or symbol results.

pub(crate) mod cursor;
pub(crate) mod declaration;
