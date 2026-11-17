//! Prepares analysis views and source context for one request.
//!
//! A snapshot borrows the project and query context. File routing selects a crate interpretation;
//! reference-search planning narrows the crates and files a query needs to scan.

mod reference_search;
mod snapshot;
mod source;

pub use self::{snapshot::ProjectSnapshot, source::FileContext};
