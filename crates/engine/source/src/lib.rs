//! Exact Rust source inputs for one frozen analysis generation.
//!
//! A project build may take long enough for files on disk to change underneath it. If each
//! analysis provider reads those files independently, declarations and bodies can describe
//! different revisions. Ajika inputs capture each source revision and path probe. The host
//! verifies those observations before publishing a new generation. An evicted source can be
//! reloaded only if the bytes still match the captured revision.
//!
//!
mod captured;
mod entry;
mod error;
mod inputs;
mod read_error;
mod revision;

use rg_std::CancellationToken;

pub use self::{
    captured::CapturedSource,
    entry::{SourceEntry, read_source_text},
    error::SourceError,
    inputs::{SourceInput, SourcePresence},
    read_error::ReadError,
    revision::SourceRevision,
};

/// All providers in one project use the same host cancellation signal. It is a service so
/// replacing a request's signal does not change the captured source revision.
#[derive(Default)]
pub struct QueryServices {
    pub cancellation: CancellationToken,
}

impl ajika::Services for QueryServices {
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

pub type QueryContext<'db> = ajika::Db<'db, QueryServices>;
pub type QueryDatabase = ajika::Database<QueryServices>;

#[cfg(test)]
mod tests;
