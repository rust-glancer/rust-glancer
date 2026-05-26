//! Generic read-only projections over the frozen index stores.
//!
//! Views are the internal index API: they may compose DefMap, Semantic IR, and Body IR, but their
//! public methods should describe indexed facts rather than one IDE feature's workflow. Completion,
//! hover, symbols, and other query modules should build feature-specific behavior on top of these
//! generic projections.

pub(crate) mod declaration;
pub(crate) mod details;
pub(crate) mod enum_variant;
pub(crate) mod implementation;
pub(crate) mod member;
pub(crate) mod reference;
pub(crate) mod ty;
