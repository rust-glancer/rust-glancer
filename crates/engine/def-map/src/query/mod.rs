//! Queries over frozen def-map data.

mod cursor;
pub(crate) mod path_resolution;

pub use self::{cursor::DefMapCursorCandidate, path_resolution::ResolvePathResult};
