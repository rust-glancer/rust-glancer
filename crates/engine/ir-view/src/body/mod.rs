//! Body-owned source and semantic projections.
//!
//! Lowered Body IR is the anchor for source structure, expression types, and lexical scopes. The
//! views in this module either expose those facts directly or compose the body with the shared
//! name/type context needed to resolve paths, enum variants, methods, and associated items.

mod resolution;
mod structure;

pub(crate) use resolution::BodyResolutionView;

pub use structure::{
    BodyClosingBraceBlock, BodyClosingBraceBlockKind, BodyStructureView, MethodChainExprTy,
};
