//! Queries over frozen def-map data.

mod cursor;
mod def_map_query;
mod path_completion;
mod path_resolution;
mod resolution_env;

pub use self::{
    cursor::DefMapCursorCandidate,
    def_map_query::{DefMapQuery, DefMapSource},
    path_completion::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
    path_resolution::{
        GlobImportSource, ImportResolution, ImportedScopeBinding, ResolvePathResult, ScopeResolver,
    },
    resolution_env::{MacroDefinitionEnv, ScopeResolutionEnv, TargetResolutionEnv},
};
