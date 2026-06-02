//! Source-level facts and locations from indexed data.

mod completion;
mod occurrence;

pub use completion::{
    IndexedMemberAccessSite, IndexedNameNamespace, IndexedQualifiedPathScope,
    IndexedQualifiedPathSite, IndexedRecordFieldListSite, IndexedUnqualifiedNameScope,
    IndexedUnqualifiedNameSite, SourceCompletionView,
};
pub use occurrence::{
    IndexedSourceFact, IndexedSourceOccurrence, IndexedSourceRole, IndexedTypePathScope,
    SourceOccurrenceView,
};
