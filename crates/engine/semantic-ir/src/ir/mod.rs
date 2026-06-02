//! Semantic IR domain model.

pub(crate) mod package;
pub(crate) mod stats;

pub use self::{package::PackageIr, stats::SemanticIrStats};
