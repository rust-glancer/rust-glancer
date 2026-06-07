//! Shared path-segment scanning for body-local cursor candidates.
//!
//! Path scanners provide reusable type-path and value-path traversal for point
//! queries and whole-target scans after those queries choose their scope.

use rg_ir_model::{BodyRef, Path, ScopeId};
use rg_item_tree::{FieldKey, TypePath};
use rg_parse::{FileId, Span};

use crate::{BodyPath, ExprKind, PatData, ResolvedBodyData};

use super::{
    super::{BodyCursorCandidate, ValueReferenceSource, ValueReferenceSurface},
    sites::BodyScanSites,
};

/// Adds type-namespace path candidates from body-local type annotations.
pub(super) struct TypePathCursorScanner<'a> {
    pub(super) body_ref: BodyRef,
    pub(super) body: &'a ResolvedBodyData,
    pub(super) file_id: Option<FileId>,
    pub(super) offset: Option<u32>,
    pub(super) candidates: &'a mut Vec<BodyCursorCandidate>,
}

impl TypePathCursorScanner<'_> {
    /// Scans body-local type annotations that can contain navigable type paths.
    pub(super) fn scan(&mut self) {
        let sites = BodyScanSites::new(self.body);
        sites.walk_type_paths(self.file_id, |site| {
            self.scan_type_path(site.scope, site.path, site.file_id);
        });
    }

    /// Adds one candidate per path segment so each prefix can resolve independently.
    fn scan_type_path(&mut self, scope: ScopeId, path: &TypePath, file_id: FileId) {
        for (idx, segment) in path.segments.iter().enumerate() {
            if self.offset_matches(segment.span) {
                self.candidates.push(BodyCursorCandidate::TypePath {
                    body: self.body_ref,
                    scope,
                    path: Path::from_type_path_prefix(path, idx),
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

/// Adds value-namespace path candidates from body-local expressions and patterns.
pub(super) struct ValuePathCursorScanner<'a> {
    pub(super) body_ref: BodyRef,
    pub(super) body: &'a ResolvedBodyData,
    pub(super) file_id: Option<FileId>,
    pub(super) offset: Option<u32>,
    pub(super) include_single_segment: bool,
    pub(super) candidates: &'a mut Vec<BodyCursorCandidate>,
}

impl ValuePathCursorScanner<'_> {
    /// Scans every source form where a body-local value path can appear.
    pub(super) fn scan(&mut self) {
        // Expression source-node lookup deliberately picks one smallest AST-ish node. Qualified
        // paths need finer granularity: in `Action::Start()`, `Action` and `Start` should produce
        // different symbols even though they belong to the same lowered expression.
        for (_expr, expr_data) in self.body.exprs_with_ids() {
            if !self.file_matches(expr_data.source.file_id) {
                continue;
            }
            match &expr_data.kind {
                ExprKind::Path { path }
                | ExprKind::Record {
                    path: Some(path), ..
                } => {
                    self.scan_body_path(expr_data.scope, path, expr_data.source.file_id, false);
                }
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

    /// Visits value paths directly owned by one pattern node.
    fn scan_pat_data(&mut self, scope: ScopeId, data: &PatData) {
        if let Some(path) = data.kind.value_path() {
            self.scan_body_path(
                scope,
                path,
                data.source.file_id,
                self.include_single_segment,
            );
        }
    }

    /// Adds one candidate per value path segment so associated items and variants stay distinct.
    fn scan_body_path(
        &mut self,
        scope: ScopeId,
        path: &BodyPath,
        file_id: FileId,
        include_single_segment: bool,
    ) {
        // Expression paths already have an expression candidate for single-segment names. Segment
        // candidates are only needed for qualified expressions or for pattern paths, which do not
        // have expression ids of their own.
        if path.segment_count() <= 1 && !include_single_segment {
            return;
        }

        for idx in 0..path.segment_count() {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if self.offset_matches(span) {
                let Some(path) = path.prefix_through(idx) else {
                    continue;
                };
                self.candidates.push(BodyCursorCandidate::ValueReference {
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

    /// Shorthand record fields are source-level value uses even though there is no child
    /// expression node to attach a regular `Expr` candidate to.
    fn scan_record_expr_shorthand_values(
        &mut self,
        scope: ScopeId,
        fields: &[crate::RecordExprField],
        file_id: FileId,
    ) {
        for field in fields {
            if field.syntax.is_explicit() || !self.offset_matches(field.key_span) {
                continue;
            }
            let FieldKey::Named(name) = &field.key else {
                continue;
            };
            self.candidates.push(BodyCursorCandidate::ValueReference {
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

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
