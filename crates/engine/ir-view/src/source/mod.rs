//! Source-level occurrences and completion sites composed from indexed domain data.
//!
//! The storage crates expose the source facts inherent to definitions, signatures, and bodies.
//! This module interprets those facts into one facade vocabulary, so editor workflows do not
//! depend on storage-shaped scanners or intermediate candidate enums. For syntax too incomplete to
//! survive lowering, the completion view can also attach a request-local spelling and replacement
//! span to the nearest indexed scope.

mod completion;
mod documentation;
mod format;
mod macro_edits;
mod occurrence;
mod references;
mod scan;

#[cfg(test)]
mod tests;

pub use rg_def_map::SourceOccurrenceLocation;

pub use self::{
    completion::{
        IndexedAssociatedPathQualifier, IndexedAssociatedTypeBindingScope,
        IndexedAssociatedTypeBindingSite, IndexedMemberAccessSite, IndexedModuleFileBase,
        IndexedModuleSourceSite, IndexedPatternCompletionKind, IndexedQualifiedPathContext,
        IndexedQualifiedPathScope, IndexedQualifiedPathSite, IndexedRecordFieldListSite,
        IndexedRecordOwner, IndexedSignatureTypeSite, IndexedTraitImplSite,
        IndexedTypeNamePosition, IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope,
        IndexedUnqualifiedNameSite, SourceCompletionView,
    },
    documentation::{
        DocumentationDeclarationIndex, DocumentationPlacement, DocumentationSource,
        DocumentationSourceView, ModuleDocumentationSource,
    },
    format::FormatStringSourceView,
    macro_edits::{MacroCopyEvidence, MacroEditSourceView},
    occurrence::{
        IndexedSignatureTypeScope, IndexedSourceFact, IndexedSourceOccurrence,
        IndexedSourceProvenance, IndexedSourceRole, IndexedSourceSurface, IndexedTypePath,
        IndexedTypePathScope, SourceOccurrenceView,
    },
    references::{ReferenceMacroTargets, ReferenceSearchLabel, ReferenceSourceSelection},
};
