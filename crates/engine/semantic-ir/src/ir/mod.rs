//! Semantic IR domain model.

pub(crate) mod package;
pub(crate) mod resolution;
pub(crate) mod stats;

pub use self::{package::PackageIr, resolution::TypePathContext, stats::SemanticIrStats};
