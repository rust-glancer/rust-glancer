//! Shared path-segment scanning for body-local cursor candidates.
//!
//! Path scanners provide reusable type-path and value-path traversal for point
//! queries and whole-target scans after those queries choose their scope.

use rg_def_map::Path;
use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};
use rg_parse::{FileId, Span};

use crate::{BodyData, BodyPath, BodyRef, ExprKind, PatId, PatKind, ScopeId, StmtKind};

use super::super::BodyCursorCandidate;

/// Adds type-namespace path candidates from body-local type annotations.
pub(super) struct TypePathCursorScanner<'a> {
    pub(super) body_ref: BodyRef,
    pub(super) body: &'a BodyData,
    pub(super) file_id: Option<FileId>,
    pub(super) offset: Option<u32>,
    pub(super) candidates: &'a mut Vec<BodyCursorCandidate>,
}

impl TypePathCursorScanner<'_> {
    /// Scans let annotations; these are the body-local places that carry type paths today.
    pub(super) fn scan(&mut self) {
        for statement in self.body.statements.iter() {
            if !self.file_matches(statement.source.file_id) {
                continue;
            }
            let StmtKind::Let {
                scope,
                annotation: Some(annotation),
                ..
            } = &statement.kind
            else {
                continue;
            };
            self.scan_type_ref(*scope, annotation, statement.source.file_id);
        }
    }

    /// Recurses through a type reference and visits every nested path-like type.
    fn scan_type_ref(&mut self, scope: ScopeId, ty: &TypeRef, file_id: FileId) {
        match ty {
            TypeRef::Path(path) => self.scan_type_path(scope, path, file_id),
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.scan_type_ref(scope, ty, file_id);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.scan_type_ref(scope, inner, file_id),
            TypeRef::Array { inner, .. } => self.scan_type_ref(scope, inner, file_id),
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.scan_type_ref(scope, param, file_id);
                }
                self.scan_type_ref(scope, ret, file_id);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    if let TypeBound::Trait(ty) = bound {
                        self.scan_type_ref(scope, ty, file_id);
                    }
                }
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
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

            for arg in &segment.args {
                self.scan_generic_arg(scope, arg, file_id);
            }
        }
    }

    fn scan_generic_arg(&mut self, scope: ScopeId, arg: &GenericArg, file_id: FileId) {
        match arg {
            GenericArg::Type(ty) => self.scan_type_ref(scope, ty, file_id),
            GenericArg::AssocType { ty: Some(ty), .. } => {
                self.scan_type_ref(scope, ty, file_id);
            }
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}

/// Adds value-namespace path candidates from body-local expressions and patterns.
pub(super) struct ValuePathCursorScanner<'a> {
    pub(super) body_ref: BodyRef,
    pub(super) body: &'a BodyData,
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
        for (_expr, expr_data) in self.body.exprs.iter_with_ids() {
            if !self.file_matches(expr_data.source.file_id) {
                continue;
            }
            if let ExprKind::Path { path } = &expr_data.kind {
                self.scan_body_path(expr_data.scope, path, expr_data.source.file_id);
            }
        }

        // Pattern paths are not represented as expressions, but they are still editor-visible
        // value paths: `let Some(value) = option` and `Action::Start { .. }` should navigate from
        // both the enum name and the variant name.
        for statement in self.body.statements.iter() {
            if !self.file_matches(statement.source.file_id) {
                continue;
            }
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                ..
            } = &statement.kind
            else {
                continue;
            };
            self.scan_pat(*scope, *pat);
        }

        for expr in self.body.exprs.iter() {
            if !self.file_matches(expr.source.file_id) {
                continue;
            }
            match &expr.kind {
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            self.scan_pat(arm.scope, pat);
                        }
                    }
                }
                ExprKind::Let {
                    scope,
                    pat: Some(pat),
                    ..
                }
                | ExprKind::For {
                    scope,
                    pat: Some(pat),
                    ..
                } => self.scan_pat(*scope, *pat),
                _ => {}
            }
        }
    }

    /// Recurses through nested patterns and visits value paths they expose.
    fn scan_pat(&mut self, scope: ScopeId, pat: PatId) {
        let Some(data) = self.body.pat(pat) else {
            return;
        };

        match &data.kind {
            PatKind::TupleStruct { path, fields } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
                for field in fields {
                    self.scan_pat(scope, *field);
                }
            }
            PatKind::Record { path, fields, .. } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
                for field in fields {
                    self.scan_pat(scope, field.pat);
                }
            }
            PatKind::Path { path } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
            }
            PatKind::Binding {
                binding,
                path,
                subpat,
                ..
            } => {
                if binding.is_none()
                    && let Some(path) = path
                {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
                if let Some(subpat) = subpat {
                    self.scan_pat(scope, *subpat);
                }
            }
            PatKind::Tuple { fields }
            | PatKind::Or { pats: fields }
            | PatKind::Slice { fields } => {
                for field in fields {
                    self.scan_pat(scope, *field);
                }
            }
            PatKind::Ref { pat, .. } | PatKind::Box { pat } => self.scan_pat(scope, *pat),
            PatKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.scan_pat(scope, *start);
                }
                if let Some(end) = end {
                    self.scan_pat(scope, *end);
                }
            }
            PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => {}
        }
    }

    /// Adds one candidate per value path segment so associated items and variants stay distinct.
    fn scan_body_path(&mut self, scope: ScopeId, path: &BodyPath, file_id: FileId) {
        // Single-segment expression paths are already represented by the surrounding expression
        // node. Segment candidates are only needed when the cursor can mean a prefix or an
        // associated item/variant inside one qualified path.
        if path.segment_count() <= 1 && !self.include_single_segment {
            return;
        }

        for idx in 0..path.segment_count() {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if self.offset_matches(span) {
                self.candidates.push(BodyCursorCandidate::ValuePath {
                    body: self.body_ref,
                    scope,
                    path: path.prefix_through(idx),
                    file_id,
                    span,
                });
            }
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
