//! Shared path-segment scanning for body-local source occurrences.
//!
//! Path scanners provide reusable type-path and value-path traversal for point
//! queries and whole-crate scans after those queries choose their scope.
//! Each written segment is represented by the prefix needed to resolve that segment:
//!
//! ```text
//! crate::model::User
//! ^^^^^  -> `crate`
//!        ^^^^^ -> `crate::model`
//!               ^^^^ -> `crate::model::User`
//! ```

use rg_ir_model::{BodyRef, FieldKey, Path, ScopeId};
use rg_item_tree::TypePath;
use rg_parse::{FileId, Span};

use rg_body_ir::{BodyPath, BodyView, ExprKind, PatData, RecordExprField};

use super::{
    BodySourceCandidate, ValueReferenceSource, ValueReferenceSurface, sites::BodyScanSites,
};

/// Adds type-namespace path candidates from body-local type syntax.
///
/// This includes nested paths such as both `Outer` and `Inner` in `Outer<Inner>`, not only the
/// outer annotation.
pub(super) struct TypePathSourceScanner<'a> {
    body_ref: BodyRef,
    body: BodyView<'a>,
    file_id: Option<FileId>,
    offset: Option<u32>,
    candidates: &'a mut Vec<BodySourceCandidate>,
}

impl<'a> TypePathSourceScanner<'a> {
    pub(super) fn at(
        body_ref: BodyRef,
        body: BodyView<'a>,
        file_id: FileId,
        offset: u32,
        candidates: &'a mut Vec<BodySourceCandidate>,
    ) -> Self {
        Self {
            body_ref,
            body,
            file_id: Some(file_id),
            offset: Some(offset),
            candidates,
        }
    }

    pub(super) fn in_crate(
        body_ref: BodyRef,
        body: BodyView<'a>,
        file_id: Option<FileId>,
        candidates: &'a mut Vec<BodySourceCandidate>,
    ) -> Self {
        Self {
            body_ref,
            body,
            file_id,
            offset: None,
            candidates,
        }
    }

    /// Scans body-local type annotations that can contain navigable type paths.
    pub(super) fn scan(&mut self) {
        let sites = BodyScanSites::new(self.body);
        sites.walk_type_paths(self.file_id, |site| {
            self.scan_type_path(site.scope, site.path, site.file_id);
        });
    }

    /// Adds one candidate per path segment so each prefix can resolve independently.
    fn scan_type_path(&mut self, scope: ScopeId, path: &TypePath, file_id: FileId) {
        if path.anchor.is_some() {
            return;
        }

        for (idx, segment) in path.segments.iter().enumerate() {
            if self.offset_matches(segment.span) {
                let Some(path) = path.as_def_map_path_prefix(idx) else {
                    continue;
                };
                self.candidates.push(BodySourceCandidate::TypePath {
                    body: self.body_ref,
                    scope,
                    path,
                    file_id,
                    span: segment.span,
                });
            }
        }
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}

/// Adds path candidates from body-local expressions and patterns.
///
/// Qualifier segments are type/module-looking, while the final segment normally occupies the
/// value namespace:
///
/// ```text
/// Action::Start
/// ^^^^^^ type-path candidate
///         ^^^^^ value-path candidate
/// ```
///
/// Record syntax splits the same-looking path differently. In `model::User { id }`, the lowered
/// record expression owns `User` and this scanner keeps only the `model` qualifier. In
/// `let model::User { id } = user`, there is no record expression, so the pattern path owns both
/// segments.
pub(super) struct BodyPathSourceScanner<'a> {
    body_ref: BodyRef,
    body: BodyView<'a>,
    file_id: Option<FileId>,
    offset: Option<u32>,
    include_single_segment: bool,
    candidates: &'a mut Vec<BodySourceCandidate>,
}

/// Selects which source fact owns the final segment after qualifiers have been emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyPathFinalSegment {
    /// A lowered expression owns the final name.
    ///
    /// For `model::User { id }`, the record expression represents `User`; this scanner emits only
    /// the `model` qualifier.
    Expression,
    /// The final name belongs to a type path.
    ///
    /// For `let model::User { id } = user`, this scanner emits both `model` and `User` because a
    /// record pattern has no expression id.
    TypePath,
    /// The final name belongs to a value path.
    ///
    /// For `let action = Action::Start`, `Action` is a type-path qualifier and `Start` is the value
    /// reference selected by this case.
    ValuePath,
}

impl<'a> BodyPathSourceScanner<'a> {
    pub(super) fn at(
        body_ref: BodyRef,
        body: BodyView<'a>,
        file_id: FileId,
        offset: u32,
        candidates: &'a mut Vec<BodySourceCandidate>,
    ) -> Self {
        Self {
            body_ref,
            body,
            file_id: Some(file_id),
            offset: Some(offset),
            include_single_segment: false,
            candidates,
        }
    }

    pub(super) fn in_crate(
        body_ref: BodyRef,
        body: BodyView<'a>,
        file_id: Option<FileId>,
        candidates: &'a mut Vec<BodySourceCandidate>,
    ) -> Self {
        Self {
            body_ref,
            body,
            file_id,
            offset: None,
            include_single_segment: true,
            candidates,
        }
    }

    /// Scans every source form where a body-local constructor or value path can appear.
    pub(super) fn scan(&mut self) {
        // Expression source-node lookup deliberately picks one smallest AST-ish node. Qualified
        // paths need finer granularity: in `Action::Start()`, `Action` and `Start` should produce
        // different symbols even though they belong to the same lowered expression.
        for expr_data in self.body.exprs() {
            if !expr_data.source.is_written_in_selected_file(self.file_id) {
                continue;
            }
            match &expr_data.kind {
                ExprKind::Path { path } => self.scan_body_path(
                    expr_data.scope,
                    path,
                    expr_data.source.file_id,
                    false,
                    BodyPathFinalSegment::ValuePath,
                ),
                ExprKind::Record {
                    path: Some(path), ..
                } => self.scan_body_path(
                    expr_data.scope,
                    path,
                    expr_data.source.file_id,
                    false,
                    BodyPathFinalSegment::Expression,
                ),
                _ => {}
            }
            if let ExprKind::Record { fields, .. } = &expr_data.kind {
                self.scan_record_expr_shorthand_values(
                    expr_data.scope,
                    fields,
                    expr_data.source.file_id,
                );
            }
        }

        // Pattern paths are not represented as expressions, but they are still editor-visible
        // value paths: `let Some(value) = option` and `Action::Start { .. }` should navigate from
        // both the enum name and the variant name.
        let sites = BodyScanSites::new(self.body);
        sites.walk_pats(self.file_id, self.offset, |site| {
            self.scan_pat_data(site.scope, site.data);
        });
    }

    /// Visits constructor paths directly owned by one pattern node.
    fn scan_pat_data(&mut self, scope: ScopeId, data: &PatData) {
        if let Some(path) = data.kind.record_path() {
            self.scan_body_path(
                scope,
                path,
                data.source.file_id,
                self.include_single_segment,
                BodyPathFinalSegment::TypePath,
            );
        } else if let Some(path) = data.kind.value_path() {
            self.scan_body_path(
                scope,
                path,
                data.source.file_id,
                self.include_single_segment,
                BodyPathFinalSegment::ValuePath,
            );
        }
    }

    /// Adds one candidate per path segment so qualifiers and constructors stay distinct.
    fn scan_body_path(
        &mut self,
        scope: ScopeId,
        path: &BodyPath,
        file_id: FileId,
        include_single_segment: bool,
        final_segment: BodyPathFinalSegment,
    ) {
        // Expression paths already have an expression candidate for single-segment names. Segment
        // candidates are only needed for qualified expressions or for pattern paths, which do not
        // have expression ids of their own.
        if path.segment_count() <= 1 && !include_single_segment {
            return;
        }

        let segment_count = path.segment_count();
        for idx in 0..segment_count {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if self.offset_matches(span) {
                let Some(path) = path.prefix_through(idx) else {
                    continue;
                };
                if idx + 1 < segment_count
                    || matches!(final_segment, BodyPathFinalSegment::TypePath)
                {
                    // In `Action::Start`, the prefix is still a user-visible type/module path.
                    // Record patterns also resolve their final constructor segment as a type path.
                    self.candidates.push(BodySourceCandidate::TypePath {
                        body: self.body_ref,
                        scope,
                        path: path.clone(),
                        file_id,
                        span,
                    });
                    continue;
                }
                if matches!(final_segment, BodyPathFinalSegment::ValuePath) {
                    self.candidates.push(BodySourceCandidate::ValueReference {
                        body: self.body_ref,
                        scope,
                        file_id,
                        span,
                        source: ValueReferenceSource::Path(path),
                        surface: ValueReferenceSurface::Plain,
                    });
                }
            }
        }
    }

    /// Shorthand record fields are source-level value uses even though there is no child
    /// expression node to attach a regular `Expr` candidate to.
    fn scan_record_expr_shorthand_values(
        &mut self,
        scope: ScopeId,
        fields: &[RecordExprField],
        file_id: FileId,
    ) {
        for field in fields {
            if field.syntax.is_explicit() || !self.offset_matches(field.key_span) {
                continue;
            }
            let FieldKey::Named(name) = &field.key else {
                continue;
            };
            self.candidates.push(BodySourceCandidate::ValueReference {
                body: self.body_ref,
                scope,
                file_id,
                span: field.key_span,
                source: match field.value {
                    Some(expr) => ValueReferenceSource::Expr(expr),
                    None => ValueReferenceSource::Path(Path::unqualified_name(name.as_str())),
                },
                surface: ValueReferenceSurface::RecordExprShorthand {
                    key: field.key.clone(),
                    field_span: field.source_span,
                },
            });
        }
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
