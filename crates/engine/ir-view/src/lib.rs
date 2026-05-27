//! Generic read-only projections over the frozen index stores.
//!
//! Views are the internal index API: they may compose DefMap, Semantic IR, and Body IR, but their
//! public methods should describe indexed facts rather than one IDE feature's workflow. Completion,
//! hover, symbols, and other query modules should build feature-specific behavior on top of these
//! generic projections.

pub mod body;
pub mod db;
pub mod declaration;
pub mod details;
pub mod enum_variant;
pub mod implementation;
pub mod item_index;
pub mod kind;
pub mod member;
pub mod module;
pub mod name_lookup;
pub mod path;
pub mod reference;
pub mod resolution;
pub mod signature;
pub mod source;
pub mod ty;
pub mod ty_label;

pub use db::IndexedViewDb;
pub use kind::IndexedSymbolKind;
