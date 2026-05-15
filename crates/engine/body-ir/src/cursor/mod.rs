//! Cursor-oriented queries over lowered function bodies.
//!
//! Analysis owns the public query vocabulary, but Body IR owns body source layout: expression
//! spans, binding spans, body-local item names, let annotations, and dot-completion receiver
//! ranges. These queries are intentionally exposed only on read transactions.

mod scan;

use rg_def_map::{Path, TargetRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{
    BindingId, BodyFieldRef, BodyFunctionRef, BodyIrReadTxn, BodyItemKind, BodyItemRef, BodyRef,
    BodyTy, ExprId, ScopeId,
};

use self::scan::{
    BodyCursorScanner, BodySourceScanner, DotCompletionSiteScanner, PathCompletionSiteScanner,
    UnqualifiedCompletionSiteScanner,
};

/// Source site selected for a dot-completion query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DotCompletionSite {
    pub body: BodyRef,
    pub receiver: ExprId,
    /// Member-name prefix already typed after the dot.
    ///
    /// For a bare dot, this is an empty span at the completion offset.
    pub member_prefix_span: Span,
}

/// Namespace expected by a path-completion site inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCompletionNamespace {
    Types,
    Values,
}

/// Namespace expected by an unqualified completion site inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnqualifiedCompletionNamespace {
    Types,
    Values,
}

/// Source site selected for a qualified-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Path before the segment being completed.
    pub qualifier: Path,
    /// Segment prefix already typed after `::`.
    pub member_prefix_span: Span,
    pub namespace: PathCompletionNamespace,
}

/// Source site selected for an unqualified completion query inside a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnqualifiedCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Name prefix already typed at the cursor.
    pub member_prefix_span: Span,
    pub namespace: UnqualifiedCompletionNamespace,
    /// Number of body-wide bindings visible before this source site.
    ///
    /// Bindings are allocated in source order, so this boundary prevents later
    /// `let` declarations from completing before they are in scope.
    pub visible_bindings: usize,
}

/// Body-local name visible at an unqualified completion site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyUnqualifiedCompletionCandidate {
    Binding {
        binding: BindingId,
        label: String,
        /// Number of lexical parents between the cursor scope and the binding's scope.
        scope_distance: usize,
    },
    LocalItem {
        item: BodyItemRef,
        kind: BodyItemKind,
        label: String,
        /// Number of lexical parents between the cursor scope and the item's scope.
        scope_distance: usize,
    },
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
    LocalField {
        field: BodyFieldRef,
        span: Span,
    },
    LocalFunction {
        function: BodyFunctionRef,
        span: Span,
    },
    TypePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        file_id: FileId,
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
        file_id: FileId,
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
            | Self::LocalField { span, .. }
            | Self::LocalFunction { span, .. }
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

    /// Returns body-local source candidates in one target.
    pub fn source_candidates(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        BodySourceScanner::new(self, target, file_id).scan()
    }

    /// Returns the source site for a dot-completion query.
    pub fn dot_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<DotCompletionSite>, PackageStoreError> {
        DotCompletionSiteScanner::new(self, target, file_id, offset).site_at_dot()
    }

    /// Returns the source site for a qualified-path completion query inside a body.
    pub fn path_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<PathCompletionSite>, PackageStoreError> {
        PathCompletionSiteScanner::new(self, target, file_id, offset).site_at_path()
    }

    /// Returns the source site for an unqualified completion query inside a body.
    pub fn unqualified_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        UnqualifiedCompletionSiteScanner::new(self, target, file_id, offset).site_at_name()
    }

    /// Returns body-local names visible from an unqualified completion site.
    pub fn unqualified_completion_candidates(
        &self,
        site: UnqualifiedCompletionSite,
    ) -> Result<Vec<BodyUnqualifiedCompletionCandidate>, PackageStoreError> {
        let Some(body) = self.body_data(site.body)? else {
            return Ok(Vec::new());
        };
        let mut candidates = Vec::new();
        let mut seen_values = Vec::new();
        let mut seen_types = Vec::new();
        let mut scope = Some(site.scope);
        let mut scope_distance = 0;

        // Walk outward from the innermost scope so local shadowing naturally wins.
        while let Some(scope_id) = scope {
            let Some(scope_data) = body.scope(scope_id) else {
                break;
            };

            if matches!(site.namespace, UnqualifiedCompletionNamespace::Values) {
                for binding_id in scope_data.bindings.iter().rev().copied() {
                    if binding_id.0 >= site.visible_bindings {
                        continue;
                    }
                    let Some(binding) = body.binding(binding_id) else {
                        continue;
                    };
                    let Some(label) = binding.name.as_ref().map(ToString::to_string) else {
                        continue;
                    };
                    if seen_values.contains(&label) {
                        continue;
                    }
                    seen_values.push(label.clone());
                    candidates.push(BodyUnqualifiedCompletionCandidate::Binding {
                        binding: binding_id,
                        label,
                        scope_distance,
                    });
                }
            }

            for item_id in scope_data.local_items.iter().rev().copied() {
                let Some(item) = body.local_item(item_id) else {
                    continue;
                };
                let label = item.name.to_string();
                if seen_types.contains(&label) {
                    continue;
                }
                seen_types.push(label.clone());
                candidates.push(BodyUnqualifiedCompletionCandidate::LocalItem {
                    item: BodyItemRef {
                        body: site.body,
                        item: item_id,
                    },
                    kind: item.kind,
                    label,
                    scope_distance,
                });
            }

            scope = scope_data.parent;
            scope_distance += 1;
        }

        Ok(candidates)
    }

    /// Returns the resolved type for the receiver expression in a dot-completion site.
    pub fn receiver_ty(
        &self,
        site: DotCompletionSite,
    ) -> Result<Option<&BodyTy>, PackageStoreError> {
        Ok(self
            .body_data(site.body)?
            .and_then(|body| body.expr(site.receiver))
            .map(|expr| &expr.ty))
    }
}
