//! Name and scope queries shared by DefMap construction and frozen snapshots.
//!
//! `ScopeResolver` reads through small environment traits, so mutable fixed-point scopes and
//! persisted DefMaps use the same Rust lookup rules. `DefMapQuery` provides the ordinary
//! query-facing implementation for frozen data.

mod def_map_query;
mod path_resolution;
mod resolution_env;

pub use self::{
    def_map_query::{DefMapQuery, DefMapSource},
    path_resolution::{GlobImportSource, ImportResolution, ResolvePathResult, ScopeResolver},
    resolution_env::{CrateResolutionEnv, MacroDefinitionEnv, ScopeResolutionEnv},
};
