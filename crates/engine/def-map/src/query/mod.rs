//! Queries over frozen def-map data.

mod def_map_query;
mod path_resolution;
mod resolution_env;

pub use self::{
    def_map_query::{DefMapQuery, DefMapSource},
    path_resolution::{
        GlobImportSource, ImportResolution, ImportedScopeBinding, ResolvePathResult, ScopeResolver,
    },
    resolution_env::{CrateResolutionEnv, MacroDefinitionEnv, ScopeResolutionEnv},
};
