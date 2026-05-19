//! Adapted from rust-analyzer's `mbe` crate.
//!
//! Types representing the three basic "styles" of macro calls in Rust source:
//! - Function-like macros ("bang macros"), e.g. `foo!(...)`
//! - Attribute macros, e.g. `#[foo]`
//! - Derive macros, e.g. `#[derive(Foo)]`

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroCallStyle {
    FnLike,
    Attr,
    Derive,
}
