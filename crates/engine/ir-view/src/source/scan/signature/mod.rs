//! Source scanning over semantic item signatures.
//!
//! Semantic IR owns item signature data, while the indexed view owns the source interpretation of
//! that data. This scanner finds fields, variants, associated functions, and nested type paths
//! without making source-query APIs part of Semantic IR's storage transaction.
//!
//! ```text
//! struct Wrapper<T> {
//!     value: outer::Inner<Vec<T>>,
//!     ^^^^^  ^^^^^  ^^^^^ ^^^ ^ fields and every nested type path remain independently navigable
//! }
//!
//! fn load<T>(value: outer::Inn$0) -> T
//!                   ^^^^^^^ completion receives qualifier `outer`, prefix span `Inn`, and `T`
//!                             remains available through the function's generic scope
//! ```

mod collector;
mod walker;

use rg_ir_model::{CrateRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, Path};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_semantic_ir::{SemanticIrReadTxn, TypePathContext};

use self::{
    collector::{SignatureCompletionCollector, SignatureOccurrenceCollector},
    walker::SignatureItemWalker,
};
use super::TypeNamePosition;

/// Semantic scope carried by a type path written in an item signature.
///
/// In `impl<T> Wrapper<T> { fn map<U>(_: U$0) {} }`, the context resolves module paths and impl
/// `Self`, while the function generic owner exposes both `U` and inherited `T`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignatureTypePathScope {
    pub(crate) context: TypePathContext,
    pub(crate) generic_owner: GenericDefRef,
}

/// Completion-shaped interpretation of one signature type path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignatureCompletionSite {
    /// A signature path such as `fn load(_: model::Us$0)` whose qualifier must be resolved.
    Qualified {
        scope: SignatureTypePathScope,
        qualifier: Path,
        member_prefix_span: Span,
    },
    /// A signature name such as `fn load<T>(_: T$0)` whose candidates come from visible scopes.
    Unqualified {
        scope: SignatureTypePathScope,
        member_prefix_span: Span,
        member_prefix: String,
        position: TypeNamePosition,
    },
}

/// One semantic signature source node that can become an indexed occurrence.
///
/// Top-level item names already come from DefMap. Semantic signatures add names owned below that
/// boundary—fields, variants, associated functions—and type paths that need their item's generic
/// and impl context for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignatureSourceCandidate {
    Field {
        field: FieldRef,
        span: Span,
    },
    Function {
        function: FunctionRef,
        span: Span,
    },
    EnumVariant {
        variant: EnumVariantRef,
        span: Span,
    },
    TypePath {
        context: TypePathContext,
        path: Path,
        file_id: FileId,
        span: Span,
    },
}

impl SignatureSourceCandidate {
    fn span(&self) -> Span {
        match self {
            Self::Field { span, .. }
            | Self::Function { span, .. }
            | Self::EnumVariant { span, .. }
            | Self::TypePath { span, .. } => *span,
        }
    }
}

/// Scans semantic item signatures for declaration names and nested type paths.
///
/// Type references are walked recursively through generic defaults, bounds, where predicates,
/// fields, parameters, return types, and qualified anchors. For `Outer<Inner>`, both paths are
/// emitted rather than treating the annotation as one opaque source span.
pub(crate) struct SignatureSourceScanner<'txn, 'db> {
    semantic_ir: &'txn SemanticIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
}

impl<'txn, 'db> SignatureSourceScanner<'txn, 'db> {
    pub(crate) fn at(
        semantic_ir: &'txn SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            semantic_ir,
            crate_ref,
            file_id: Some(file_id),
            offset: Some(offset),
        }
    }

    /// Returns the narrowest signature type path that can accept completion at the offset.
    ///
    /// `fn load(_: model::Us$0)` produces a qualified site for `model`, while
    /// `fn load<T>(_: T$0)` produces an unqualified site carrying the function's generic scope.
    pub(crate) fn completion_site_at(
        semantic_ir: &'txn SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureCompletionSite>, PackageStoreError> {
        let collector = SignatureItemWalker::new(
            semantic_ir,
            crate_ref,
            SignatureCompletionCollector::new(file_id, offset),
        )
        .scan()?;
        Ok(collector.finish())
    }

    pub(crate) fn in_crate(
        semantic_ir: &'txn SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            semantic_ir,
            crate_ref,
            file_id,
            offset: None,
        }
    }

    /// Collects the source facts owned by each semantic item family in the crate.
    pub(crate) fn scan(self) -> Result<Vec<SignatureSourceCandidate>, PackageStoreError> {
        let collector = SignatureItemWalker::new(
            self.semantic_ir,
            self.crate_ref,
            SignatureOccurrenceCollector::new(self.file_id, self.offset),
        )
        .scan()?;
        Ok(collector.finish())
    }
}
