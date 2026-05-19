//! Qualified-path completion site scanning.
//!
//! Path completion scans recognize partially typed segments in paths such as
//! `crate::module::Us` and return the qualifier, replacement span, and expected namespace.

use rg_def_map::{Path, TargetRef};
use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    BodyData, BodyId, BodyIrReadTxn, BodyPath, BodyRef, ExprKind, PatId, PatKind, ScopeId, StmtKind,
};

use super::super::{PathCompletionNamespace, PathCompletionSite};

/// Finds the source site that belongs to a qualified-path completion offset.
pub(crate) struct PathCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> PathCompletionSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            target,
            file_id,
            offset,
        }
    }

    /// Returns the smallest type or value path whose segment prefix accepts completions.
    pub(crate) fn site_at_path(&self) -> Result<Option<PathCompletionSite>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(PathCompletionSite, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            // Body spans are a cheap first filter before scanning every expression and statement.
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            self.scan_type_paths(body_ref, body, &mut best);
            self.scan_value_paths(body_ref, body, &mut best);
        }

        Ok(best.map(|(site, _)| site))
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_paths(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        for statement in body.statements.iter() {
            if statement.source.file_id != self.file_id {
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
            self.scan_type_ref(body_ref, *scope, annotation, best);
        }

        for expr in body.exprs.iter() {
            if expr.source.file_id != self.file_id {
                continue;
            }
            let ExprKind::Closure {
                scope,
                params,
                ret_ty,
                ..
            } = &expr.kind
            else {
                continue;
            };
            for param in params {
                if let Some(annotation) = &param.annotation {
                    self.scan_type_ref(body_ref, *scope, annotation, best);
                }
            }
            if let Some(ret_ty) = ret_ty {
                self.scan_type_ref(body_ref, *scope, ret_ty, best);
            }
        }
    }

    /// Recurses through type syntax because the completion site may be in a generic argument.
    fn scan_type_ref(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        ty: &TypeRef,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        match ty {
            TypeRef::Path(path) => {
                if let Some(site) = self.site_for_type_path(body_ref, scope, path) {
                    Self::remember_site(site, path.source_span.len(), best);
                }

                for segment in &path.segments {
                    for arg in &segment.args {
                        self.scan_generic_arg(body_ref, scope, arg, best);
                    }
                }
            }
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.scan_type_ref(body_ref, scope, ty, best);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.scan_type_ref(body_ref, scope, inner, best),
            TypeRef::Array { inner, .. } => {
                self.scan_type_ref(body_ref, scope, inner, best);
            }
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.scan_type_ref(body_ref, scope, param, best);
                }
                self.scan_type_ref(body_ref, scope, ret, best);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    if let TypeBound::Trait(ty) = bound {
                        self.scan_type_ref(body_ref, scope, ty, best);
                    }
                }
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    fn scan_generic_arg(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        arg: &GenericArg,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        match arg {
            GenericArg::Type(ty) => self.scan_type_ref(body_ref, scope, ty, best),
            GenericArg::AssocType { ty: Some(ty), .. } => {
                self.scan_type_ref(body_ref, scope, ty, best);
            }
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    /// Scans expression and pattern paths where value-namespace completions can appear.
    fn scan_value_paths(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        for (_expr, expr_data) in body.exprs.iter_with_ids() {
            if expr_data.source.file_id != self.file_id {
                continue;
            }
            if let ExprKind::Path { path } = &expr_data.kind {
                self.scan_body_path(body_ref, expr_data.scope, path, best);
            }
        }

        for statement in body.statements.iter() {
            if statement.source.file_id != self.file_id {
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
            self.scan_pat(body_ref, body, *scope, *pat, best);
        }

        for expr in body.exprs.iter() {
            if expr.source.file_id != self.file_id {
                continue;
            }
            match &expr.kind {
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            self.scan_pat(body_ref, body, arm.scope, pat, best);
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
                } => self.scan_pat(body_ref, body, *scope, *pat, best),
                ExprKind::Closure { scope, params, .. } => {
                    for param in params {
                        if let Some(pat) = param.pat {
                            self.scan_pat(body_ref, body, *scope, pat, best);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Recurses through nested patterns and visits value paths they expose.
    fn scan_pat(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        scope: ScopeId,
        pat: PatId,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        let Some(data) = body.pat(pat) else {
            return;
        };

        match &data.kind {
            PatKind::TupleStruct { path, fields } => {
                if let Some(path) = path {
                    self.scan_body_path(body_ref, scope, path, best);
                }
                for field in fields {
                    self.scan_pat(body_ref, body, scope, *field, best);
                }
            }
            PatKind::Record { path, fields, .. } => {
                if let Some(path) = path {
                    self.scan_body_path(body_ref, scope, path, best);
                }
                for field in fields {
                    self.scan_pat(body_ref, body, scope, field.pat, best);
                }
            }
            PatKind::Path { path } => {
                if let Some(path) = path {
                    self.scan_body_path(body_ref, scope, path, best);
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
                    self.scan_body_path(body_ref, scope, path, best);
                }
                if let Some(subpat) = subpat {
                    self.scan_pat(body_ref, body, scope, *subpat, best);
                }
            }
            PatKind::Tuple { fields }
            | PatKind::Or { pats: fields }
            | PatKind::Slice { fields } => {
                for field in fields {
                    self.scan_pat(body_ref, body, scope, *field, best);
                }
            }
            PatKind::Ref { pat, .. } | PatKind::Box { pat } => {
                self.scan_pat(body_ref, body, scope, *pat, best);
            }
            PatKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.scan_pat(body_ref, body, scope, *start, best);
                }
                if let Some(end) = end {
                    self.scan_pat(body_ref, body, scope, *end, best);
                }
            }
            PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => {}
        }
    }

    /// Finds a partially typed type path segment after at least one qualifier segment.
    fn site_for_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &TypePath,
    ) -> Option<PathCompletionSite> {
        for (idx, segment) in path.segments.iter().enumerate().skip(1) {
            if !segment.span.touches(self.offset) {
                continue;
            }

            return Some(PathCompletionSite {
                body,
                scope,
                qualifier: Path::from_type_path_prefix(path, idx - 1),
                member_prefix_span: segment.span,
                namespace: PathCompletionNamespace::Types,
            });
        }

        let last_segment = path.segments.last()?;
        // Generic argument text also extends past the segment name. When arguments are present,
        // that suffix is not the synthetic empty segment created by a trailing `::`.
        if !last_segment.args.is_empty() {
            return None;
        }
        let span = self.empty_member_span(path.source_span, last_segment.span)?;

        // Live edits such as `let value: crate::` have no final segment yet. Treat the completed
        // prefix as the qualifier and let completion fill the missing segment.
        Some(PathCompletionSite {
            body,
            scope,
            qualifier: Path::from_type_path(path),
            member_prefix_span: span,
            namespace: PathCompletionNamespace::Types,
        })
    }

    /// Returns an empty replacement span when the cursor sits after a trailing `::`.
    fn empty_member_span(&self, source_span: Span, last_segment_span: Span) -> Option<Span> {
        let has_trailing_separator = source_span.text.end == last_segment_span.text.end + 2;
        if !has_trailing_separator {
            return None;
        }

        let offset_after_last_segment =
            last_segment_span.text.end <= self.offset && self.offset <= source_span.text.end;
        if !offset_after_last_segment {
            return None;
        }

        Some(Span {
            text: TextSpan {
                start: self.offset,
                end: self.offset,
            },
        })
    }

    fn empty_member_site_for_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &BodyPath,
    ) -> Option<PathCompletionSite> {
        let last_segment_span = path.segment_span(path.segment_count().checked_sub(1)?)?;
        let span = self.empty_member_span(path.source_span, last_segment_span)?;

        // Expression and pattern paths can use modules and types as intermediate qualifiers, even
        // when the final completed path must eventually denote a value.
        Some(PathCompletionSite {
            body,
            scope,
            qualifier: path.prefix_through(path.segment_count() - 1),
            member_prefix_span: span,
            namespace: PathCompletionNamespace::Values,
        })
    }

    /// Finds a partially typed value path segment after at least one qualifier segment.
    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &BodyPath,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        for idx in 1..path.segment_count() {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if !span.touches(self.offset) {
                continue;
            }

            Self::remember_site(
                PathCompletionSite {
                    body,
                    scope,
                    qualifier: path.prefix_through(idx - 1),
                    member_prefix_span: span,
                    namespace: PathCompletionNamespace::Values,
                },
                path.source_span.len(),
                best,
            );
        }

        if let Some(site) = self.empty_member_site_for_body_path(body, scope, path) {
            Self::remember_site(site, path.source_span.len(), best);
        }
    }

    /// Keeps nested path behavior predictable by choosing the shortest matching path syntax.
    fn remember_site(
        site: PathCompletionSite,
        source_len: u32,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        if best
            .as_ref()
            .is_none_or(|(_, best_len)| source_len < *best_len)
        {
            *best = Some((site, source_len));
        }
    }
}
