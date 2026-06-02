//! Queries over frozen def-map data.

mod cursor;
mod def_map_query;
mod path_completion;
pub(crate) mod path_resolution;
pub(crate) mod resolution_env;

pub use self::{
    cursor::DefMapCursorCandidate,
    def_map_query::{DefMapQuery, DefMapSource},
    path_completion::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
    path_resolution::{NameResolutionFilter, ResolvePathResult},
};
