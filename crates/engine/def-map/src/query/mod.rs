//! Queries over frozen def-map data.

mod cursor;
mod path_completion;
pub(crate) mod path_resolution;
pub(crate) mod resolution_env;

pub use self::{
    cursor::DefMapCursorCandidate,
    path_completion::{
        DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite, ScopeNamespace, VisibleScopeDef,
        VisibleScopeOrigin,
    },
    path_resolution::{NameResolutionFilter, ResolvePathResult},
};
