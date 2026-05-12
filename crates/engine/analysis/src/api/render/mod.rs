//! Presentation renderers for analysis-owned UI payloads.
//!
//! Renderers turn stable IR identities and type facts into compact Rust-like labels for hover,
//! inlay hints, and related editor surfaces. They stay inside analysis so callers receive strings
//! ready for transport mapping without depending on IR formatting details.

pub(crate) mod path;
pub(crate) mod signature;
pub(crate) mod ty;
