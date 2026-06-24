//! Shared declarative macro compilation and expansion support.
//!
//! The reusable runtime machinery lives in `rg_macro_runtime`; this module keeps the body-facing
//! facade that still depends on frozen def-map visibility.

mod body;
pub(crate) mod builtin;

pub use self::body::{BodyMacroExpander, BodyMacroExprExpansion, ExpandedBodyMacro};
