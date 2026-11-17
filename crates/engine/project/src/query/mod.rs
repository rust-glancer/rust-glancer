//! Prepares analysis views and source context for one request.
//!
//! A snapshot borrows the accepted project and the active query operation. Request-local
//! current-source preparation adds the captured editor text and bodies needed by that request.

mod current_source;
mod reference_search;
mod snapshot;
mod source;

pub use self::{
    current_source::DocumentSourceView, snapshot::ProjectSnapshot, source::FileContext,
};
