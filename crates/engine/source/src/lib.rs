//! Exact Rust source inputs for one frozen analysis generation.
//!
//! A project build may take long enough for files on disk to change underneath it. If each
//! analysis phase reads those files independently, ItemTree can describe one version of a file
//! while Body IR describes another. This crate puts one source boundary in front of those phases.
//!
//! The normal lifecycle is:
//!
//! 1. An open [`SourceInventory`] captures source files and module-existence decisions.
//! 2. Every captured [`SourceEntry`] records the exact byte [`SourceRevision`] used by the build.
//! 3. File discovery finishes and the inventory is sealed.
//! 4. The candidate is validated once more before its project generation is published.
//! 5. Saved text may be evicted. A later reload is accepted only when it still has the captured
//!    revision.
//!
//! Captured override text follows the same entry API, but it has no filesystem authority. It stays
//! resident for the lifetime of the derived project that owns it.

mod captured;
mod entry;
mod error;
mod inventory;
mod path;
mod revision;

pub use self::{
    captured::CapturedSource,
    entry::{SourceEntry, read_source_text},
    error::SourceError,
    inventory::SourceInventory,
    path::SourcePath,
    revision::{SourceDescriptor, SourceRevision},
};

#[cfg(test)]
mod tests;
