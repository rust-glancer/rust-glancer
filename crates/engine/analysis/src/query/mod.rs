//! Editor-facing analysis query implementations.
//!
//! Each module backs one public `Analysis` operation or a tightly related group of operations.
//! Queries combine generic indexed views with exact request source where the editor operation
//! needs it, then produce the transport-neutral models exported by `model`. Reusable semantic and
//! source traversal stays in `rg_ir_view`; protocol conversion stays in the LSP crates.

pub(crate) mod code_action;
pub(crate) mod completion;
pub(crate) mod folding;
pub(crate) mod hover;
pub(crate) mod import;
pub(crate) mod inlay_hints;
pub(crate) mod navigation;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod symbols;
pub(crate) mod trait_member;
