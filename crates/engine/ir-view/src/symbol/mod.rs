//! Reusable symbol projections over indexed declarations.
//!
//! This module owns source-independent symbol enumeration: it can walk DefMap, Semantic IR, and
//! Body IR indexes to produce generic outline and workspace-symbol facts. Editor-facing layers own
//! query policy, filtering, transport names, and result models built from these facts.

mod kind;
mod model;
mod view;

pub use kind::SymbolKind;
pub use model::{IndexedSymbolEntry, SourceOutlineDeclaration, SourceOutlineNode};
pub use view::SymbolView;
