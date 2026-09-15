//! Prepares analysis views and source context for one request.
//!
//! A snapshot borrows the saved project. Its read transactions own the decoded working set, and
//! current-source preparation adds the captured editor text and bodies needed by that request.

mod current_source;
mod reference_search;
mod snapshot;
mod source;
mod txn;

pub use self::{
    current_source::DocumentSourceView, snapshot::ProjectSnapshot, source::FileContext,
};
