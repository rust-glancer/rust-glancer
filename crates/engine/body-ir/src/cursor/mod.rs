//! Cursor-oriented queries over lowered function bodies.
//!
//! Analysis owns the public query vocabulary, but Body IR owns body source layout: expression
//! spans, binding spans, body-local item names, let annotations, and dot-completion receiver
//! ranges. These queries are intentionally exposed only on read transactions.

mod scan;

use rg_def_map::{Path, TargetRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{BindingId, BodyIrReadTxn, BodyItemRef, BodyRef, BodyTy, ExprId, ScopeId};

use self::scan::{BodyCursorScanner, DotReceiverScanner};

/// Receiver expression selected for a dot-completion query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DotReceiver {
    pub body: BodyRef,
    pub receiver: ExprId,
}

/// One body source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyCursorCandidate {
    Body {
        body: BodyRef,
        span: Span,
    },
    Binding {
        body: BodyRef,
        binding: BindingId,
        span: Span,
    },
    Expr {
        body: BodyRef,
        expr: ExprId,
        span: Span,
    },
    LocalItem {
        item: BodyItemRef,
        span: Span,
    },
    TypePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
    /// A value-namespace path segment inside a body expression or pattern.
    ///
    /// Type annotations have their own candidate kind because `Self` and body-local items need
    /// type resolution. This variant is for value-looking paths such as associated functions and
    /// enum variants, where a cursor on each segment can mean a different target.
    ValuePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
}

impl BodyCursorCandidate {
    pub fn span(&self) -> Span {
        match self {
            Self::Body { span, .. }
            | Self::Binding { span, .. }
            | Self::Expr { span, .. }
            | Self::LocalItem { span, .. }
            | Self::TypePath { span, .. }
            | Self::ValuePath { span, .. } => *span,
        }
    }
}

impl BodyIrReadTxn<'_> {
    /// Returns body-local cursor candidates at `offset`, including let-annotation type paths.
    pub fn cursor_candidates(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        BodyCursorScanner::new(self, target, file_id, offset).scan()
    }

    /// Returns the receiver expression for a dot-completion site.
    pub fn receiver_at_dot(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<DotReceiver>, PackageStoreError> {
        DotReceiverScanner::new(self, target, file_id, offset).receiver_at_dot()
    }

    /// Returns the resolved type for a previously-selected dot receiver.
    pub fn receiver_ty(&self, receiver: DotReceiver) -> Result<Option<&BodyTy>, PackageStoreError> {
        Ok(self
            .body_data(receiver.body)?
            .and_then(|body| body.expr(receiver.receiver))
            .map(|expr| &expr.ty))
    }
}
