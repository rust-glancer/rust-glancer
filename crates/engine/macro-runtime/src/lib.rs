//! Runtime support for declarative macro expansion.
//!
//! `rg_macro_expand` owns the low-level matcher/transcriber engine. This crate wraps that engine
//! in project-level runtime services: compiling stored macro definitions, caching repeated calls,
//! preparing self-contained expansion work, and running that work through a dedicated executor.
//! Callers still decide which macro is visible and how generated syntax is lowered into their IR.

mod cache;
mod executor;
mod syntax;

pub use rg_macro_expand::{Edition, ExpansionParseKind, ExpansionSyntax};

pub use self::{
    cache::{
        MacroCompileRecord, MacroCompileResult, MacroExpandRecord, MacroExpansionCache,
        MacroExpansionCacheKey, PreparedMacroExpansion, PreparedMacroExpansionResult,
    },
    executor::{
        MacroExpansionExecutor, MacroExpansionJob, MacroExpansionOutput,
        MacroExpansionPerformancePreference, MacroExpansionSyntax, MacroExpansionWork,
    },
    syntax::{macro_edition, tt_span_for_parse_span},
};
