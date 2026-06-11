//! Generic read-only projections over the frozen index stores.
//!
//! Views are the internal index API: they may compose DefMap, Semantic IR, and Body IR, but their
//! public methods should describe indexed facts rather than one IDE feature's workflow. Completion,
//! hover, symbols, and other query modules should build feature-specific behavior on top of these
//! generic projections.

pub mod body;
pub mod db;
pub mod display;
pub mod implementation;
pub mod item;
pub mod lookup;
pub mod member;
pub mod source;
pub mod symbol;
#[doc(hidden)]
pub mod testonly;
pub mod ty;

pub use db::IndexedViewDb;
pub use symbol::SymbolKind;

#[cfg(test)]
mod tests;
