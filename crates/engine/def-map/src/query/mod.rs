//! Name and scope queries shared by DefMap construction and frozen snapshots.
//!
//! `NameResolver` reads through small environment traits, so mutable item scopes and
//! persisted DefMaps use the same Rust lookup rules. `DefMapQuery` provides the ordinary
//! query-facing implementation for frozen data.

mod def_map_query;
mod name_resolution;
mod resolution_env;

pub use self::{
    def_map_query::{DefMapQuery, DefMapSource},
    name_resolution::{GlobImportSource, NameResolver, ResolvePathResult},
    resolution_env::{CrateResolutionEnv, MacroDefinitionEnv, NameResolutionEnv},
};
