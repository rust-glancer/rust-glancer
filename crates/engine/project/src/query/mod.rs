//! Prepares analysis views and source context for one request.
//!
//! A snapshot borrows the accepted project and the active query operation. The
//! installed input view supplies the source text for all declarations and bodies.

mod reference_search;
mod snapshot;
mod source;

pub use self::{snapshot::ProjectSnapshot, source::FileContext};
