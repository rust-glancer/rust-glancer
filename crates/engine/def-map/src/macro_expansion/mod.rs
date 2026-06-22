//! Shared declarative macro compilation and expansion support.
//!
//! This module owns the def-map-shaped part of macro expansion: compiling stored macro
//! definitions, caching repeated expansion inputs, and running self-contained expansion jobs in a
//! bounded worker pool. Callers remain responsible for resolving macro names and applying generated
//! syntax to their own IR.

mod body;
mod cache;
mod executor;
mod syntax;

pub(crate) use self::{
    cache::{MacroCompileRecord, MacroExpandRecord, MacroExpansionCache, PreparedMacroExpansion},
    executor::{MacroExpansionExecutor, MacroExpansionJob, MacroExpansionWork},
    syntax::{macro_edition, tt_span_for_parse_span},
};

pub use self::body::BodyMacroExpander;
