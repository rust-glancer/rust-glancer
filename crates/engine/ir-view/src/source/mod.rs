//! Source-level occurrences and completion sites composed from indexed domain data.
//!
//! The storage crates expose the source facts inherent to definitions, signatures, and bodies.
//! This module interprets those facts into one facade vocabulary, so editor workflows do not
//! depend on storage-shaped scanners or intermediate candidate enums.

mod completion;
mod occurrence;
mod scan;

#[cfg(test)]
mod tests;

pub use completion::{
    IndexedMemberAccessSite, IndexedNameNamespace, IndexedQualifiedPathScope,
    IndexedQualifiedPathSite, IndexedRecordFieldListSite, IndexedUnqualifiedNameScope,
    IndexedUnqualifiedNameSite, SourceCompletionView,
};
pub use occurrence::{
    IndexedSourceFact, IndexedSourceOccurrence, IndexedSourceRole, IndexedSourceSurface,
    IndexedTypePathScope, SourceOccurrenceView,
};
