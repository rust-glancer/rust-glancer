//! Source-shape projections derived from lowered Body IR.

mod resolution;
mod structure;

pub(crate) use resolution::BodyResolutionView;

pub use structure::{
    BodyClosingBraceBlock, BodyClosingBraceBlockKind, BodyStructureView, MethodChainExprTy,
};
