//! Selection and progress for current-source preparation.

use rg_parse::TextSpan;

/// Why selected current source could not be given a semantic context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum CurrentSourceUnavailable {
    /// Neither a body-owning declaration nor an impl context could be selected at the cursor.
    #[display("the cursor has no supported body or impl declaration")]
    NoBodyAtPosition,
    /// The selected declaration has no usable saved owner or containing module.
    #[display("the current declaration has no usable semantic root")]
    NoSemanticRoot,
    /// More than one saved declaration has the same header and containing declarations.
    #[display("more than one saved semantic owner matches the current body")]
    AmbiguousSavedOwner,
}

/// Points where current-source preparation can stop after an expensive step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum CurrentSourceBuildCheckpoint {
    #[display("after current source parsing")]
    SourceParsed,
    #[display("after current body owner association")]
    OwnerAssociated,
    #[display("after current body lowering")]
    BodyLowered,
    #[display("after current body-local item collection")]
    BodyLocalItemsCollected,
    #[display("after current body-local impl header resolution")]
    ImplHeadersResolved,
    #[display("after current pattern binding resolution")]
    PatternBindingsMaterialized,
    #[display("after current body resolution")]
    BodyResolved,
    #[display("after current declaration preparation")]
    DeclarationsPrepared,
}

/// Which current bodies and declarations one request needs.
///
/// A cursor selects the innermost body that touches it and includes parser recovery for unfinished
/// code. A range uses half-open overlap and may select several bodies. Keeping these policies
/// explicit avoids pretending that a cursor is just a very short range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentSourceSelection {
    /// Select the nearest body and enclosing impl, including recovery for an unfinished cursor site.
    AtOffset(u32),
    /// Select every body whose source has a strict half-open overlap with the range.
    IntersectingRange(TextSpan),
}
