//! Queries over frozen or in-progress def-map scope graphs.

mod def_map_query;
mod path_resolution;
mod resolution_env;

pub use self::{
    def_map_query::{DefMapQuery, DefMapSource},
    path_resolution::{NameResolutionFilter, PathResolver, ResolvePathResult},
    resolution_env::{ScopeResolutionEnv, TargetResolutionEnv},
};
