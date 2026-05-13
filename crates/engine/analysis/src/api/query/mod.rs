//! Editor-facing analysis query implementations.
//!
//! Each module backs one public `Analysis` operation or a tightly related group of operations.
//! Queries combine def-map, semantic IR, and body IR facts into transport-neutral result models,
//! but leave lower-level cursor/entity normalization and presentation formatting to sibling
//! modules.

pub(crate) mod completion;
pub(crate) mod hover;
pub(crate) mod navigation;
pub(crate) mod references;
pub(crate) mod symbol_at;
pub(crate) mod symbols;
pub(crate) mod type_at;
pub(crate) mod type_hints;
