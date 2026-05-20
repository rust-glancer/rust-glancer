//! Cursor-oriented queries over lowered function bodies.
//!
//! Analysis owns the public query vocabulary, but Body IR owns body source layout: expression
//! spans, binding spans, body-local item names, let annotations, and dot-completion receiver
//! ranges. These queries are intentionally exposed only on read transactions.

mod scan;

use std::collections::HashSet;

use rg_def_map::{Path, TargetRef};
use rg_item_tree::FieldKey;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_text::Name;

use crate::{
    BindingId, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef, BodyIrReadTxn, BodyItemKind,
    BodyItemRef, BodyRef, BodyTy, BodyValueItemKind, BodyValueItemRef, ExprId, ScopeId,
};

use self::scan::{
    BodyCursorScanner, BodySourceScanner, DotCompletionSiteScanner, PathCompletionSiteScanner,
    RecordFieldCompletionSiteScanner, UnqualifiedCompletionSiteScanner,
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

/// Source site selected for a record-field completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFieldCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Struct-like path before the record field list.
    pub owner: Path,
    /// Field-name prefix already typed inside the record field list.
    pub member_prefix_span: Span,
    /// Named fields already written in this literal or pattern.
    pub existing_fields: Vec<FieldKey>,
}

/// Body-local name visible at an unqualified completion site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyUnqualifiedCompletionCandidate {
    /// Local binding introduced by a parameter or pattern, e.g. `user` in `let user = input;`.
    Binding {
        binding: BindingId,
        label: String,
        /// Number of lexical parents between the cursor scope and the binding's scope.
        scope_distance: usize,
    },
    /// Body-local type-namespace item, e.g. `User` in `fn f() { struct User; }`.
    ///
    /// In value positions, this is only emitted for items that also introduce a bare constructor.
    LocalItem {
        item: BodyItemRef,
        kind: BodyItemKind,
        label: String,
        /// Number of lexical parents between the cursor scope and the item's scope.
        scope_distance: usize,
    },
    /// Body-local value-namespace item, e.g. `DEFAULT` in `fn f() { const DEFAULT: u8 = 0; }`.
    ///
    /// This covers local `const` and `static` declarations, not `let` bindings.
    LocalValueItem {
        item: BodyValueItemRef,
        kind: BodyValueItemKind,
        label: String,
        /// Number of lexical parents between the cursor scope and the item's scope.
        scope_distance: usize,
    },
    /// Body-local free function, e.g. `helper` in `fn f() { fn helper() {} }`.
    LocalFunction {
        function: BodyFunctionRef,
        label: String,
        /// Number of lexical parents between the cursor scope and the function's scope.
        scope_distance: usize,
    },
}

/// One body source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyCursorCandidate {
    /// Function body declaration, e.g. the name in `fn use_it() { ... }`.
    Body { body: BodyRef, span: Span },
    /// Local binding introduced by a parameter or pattern, e.g. `user` in `let user = input;`.
    Binding {
        body: BodyRef,
        binding: BindingId,
        span: Span,
    },
    /// Lowered expression node, e.g. the whole `user.id()` call expression.
    Expr {
        body: BodyRef,
        expr: ExprId,
        span: Span,
    },
    /// Body-local type-namespace item, e.g. `User` in `fn f() { struct User; }`.
    ///
    /// This also covers local `enum`, `union`, `type`, and `trait` declarations.
    LocalItem { item: BodyItemRef, span: Span },
    /// Body-local value-namespace item, e.g. `DEFAULT` in `fn f() { const DEFAULT: u8 = 0; }`.
    ///
    /// This covers local `const` and `static` declarations, not `let` bindings.
    LocalValueItem { item: BodyValueItemRef, span: Span },
    /// Field declared on a body-local struct or union,
    /// e.g. `id` in `fn f() { struct User { id: Id } }`.
    LocalField { field: BodyFieldRef, span: Span },
    /// Variant declared on a body-local enum, e.g. `Start` in `enum Action { Start }`.
    LocalEnumVariant {
        variant: BodyEnumVariantRef,
        span: Span,
    },
    /// Body-local function-like item, e.g. `helper` in `fn f() { fn helper() {} }`.
    LocalFunction {
        function: BodyFunctionRef,
        span: Span,
    },
    /// Type-namespace path inside a body, e.g. `User` in `let user: User;`.
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
            | Self::LocalValueItem { span, .. }
            | Self::LocalField { span, .. }
            | Self::LocalEnumVariant { span, .. }
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

    /// Returns the source site for a record-field completion query inside a body.
    pub fn record_field_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<RecordFieldCompletionSite>, PackageStoreError> {
        RecordFieldCompletionSiteScanner::new(self, target, file_id, offset).site_at_record_field()
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
        let mut seen_values = HashSet::<Name>::new();
        let mut seen_types = HashSet::<Name>::new();
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
                    let Some(name) = binding.name.as_ref() else {
                        continue;
                    };
                    if !seen_values.insert(name.clone()) {
                        continue;
                    }
                    candidates.push(BodyUnqualifiedCompletionCandidate::Binding {
                        binding: binding_id,
                        label: name.to_string(),
                        scope_distance,
                    });
                }

                for function_id in scope_data.local_functions.iter().rev().copied() {
                    let Some(function) = body.local_function(function_id) else {
                        continue;
                    };
                    if !seen_values.insert(function.name.clone()) {
                        continue;
                    }
                    candidates.push(BodyUnqualifiedCompletionCandidate::LocalFunction {
                        function: BodyFunctionRef {
                            body: site.body,
                            function: function_id,
                        },
                        label: function.name.to_string(),
                        scope_distance,
                    });
                }

                for item_id in scope_data.local_value_items.iter().rev().copied() {
                    let Some(item) = body.local_value_item(item_id) else {
                        continue;
                    };
                    if !seen_values.insert(item.name.clone()) {
                        continue;
                    }
                    candidates.push(BodyUnqualifiedCompletionCandidate::LocalValueItem {
                        item: BodyValueItemRef {
                            body: site.body,
                            item: item_id,
                        },
                        kind: item.kind,
                        label: item.name.to_string(),
                        scope_distance,
                    });
                }
            }

            for item_id in scope_data.local_items.iter().rev().copied() {
                let Some(item) = body.local_item(item_id) else {
                    continue;
                };

                match site.namespace {
                    UnqualifiedCompletionNamespace::Values => {
                        if !item.has_value_constructor() || !seen_values.insert(item.name.clone()) {
                            continue;
                        }
                    }
                    UnqualifiedCompletionNamespace::Types => {
                        if !seen_types.insert(item.name.clone()) {
                            continue;
                        }
                    }
                }

                candidates.push(BodyUnqualifiedCompletionCandidate::LocalItem {
                    item: BodyItemRef {
                        body: site.body,
                        item: item_id,
                    },
                    kind: item.kind,
                    label: item.name.to_string(),
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
