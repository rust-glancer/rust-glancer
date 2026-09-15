//! Backs saved phase data with resident payloads or package artifacts.
//!
//! Residency decides when payloads can be released. Artifact writers keep the phase databases
//! coherent when persisting them, and loaders share one open artifact revision across phase reads.
//! The cache owns the encoded representation and filesystem operations used by both sides.

pub(crate) mod artifacts;
pub(crate) mod cache;
pub(crate) mod loaders;
pub(crate) mod residency;
