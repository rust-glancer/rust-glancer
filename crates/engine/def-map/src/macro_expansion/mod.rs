//! Shared declarative macro compilation and expansion support.
//!
//! This module owns the def-map-shaped part of macro expansion: compiling stored macro
//! definitions, caching repeated expansion inputs, and running self-contained expansion jobs in a
//! bounded worker pool. Callers remain responsible for resolving macro names and applying generated
//! syntax to their own IR.

mod cache;
mod executor;

pub(crate) use self::{
    cache::{MacroCompileRecord, MacroExpandRecord, MacroExpansionCache, PreparedMacroExpansion},
    executor::{MacroExpansionExecutor, MacroExpansionJob, MacroExpansionWork},
};
