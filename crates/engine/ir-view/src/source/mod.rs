//! Source-level occurrences and completion sites composed from indexed domain data.
//!
//! The storage crates expose the source facts inherent to definitions, signatures, and bodies.
//! This module interprets those facts into one facade vocabulary, so editor workflows do not
//! depend on storage-shaped scanners or intermediate candidate enums. For syntax too incomplete to
//! survive lowering, the completion view can also attach a request-local spelling and replacement
//! span to the nearest indexed scope.

mod completion;
mod occurrence;
mod scan;

#[cfg(test)]
mod tests;

pub use completion::{
    IndexedAssociatedPathQualifier, IndexedAssociatedTypeBindingScope,
    IndexedAssociatedTypeBindingSite, IndexedMemberAccessSite, IndexedModuleSourceSite,
    IndexedPatternCompletionKind, IndexedQualifiedPathContext, IndexedQualifiedPathScope,
    IndexedQualifiedPathSite, IndexedRecordFieldListSite, IndexedRecordOwner,
    IndexedSignatureTypeSite, IndexedTraitImplSite, IndexedTypeNamePosition,
    IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope, IndexedUnqualifiedNameSite,
    SourceCompletionView,
};
pub use occurrence::{
    IndexedSignatureTypeScope, IndexedSourceFact, IndexedSourceOccurrence, IndexedSourceRole,
    IndexedSourceSurface, IndexedTypePath, IndexedTypePathScope, SourceOccurrenceView,
};
