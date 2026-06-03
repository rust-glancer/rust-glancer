//! Queries over frozen def-map data.

mod cursor;
mod path_completion;

pub use self::{
    cursor::DefMapCursorCandidate,
    path_completion::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
};
