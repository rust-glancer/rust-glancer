//! Qualified-path completion site scanning.
//!
//! Path completion scans recognize partially typed segments in paths such as
//! `crate::module::Us` and return the qualifier, replacement span, and expected namespace.

use rg_def_map::{Path, TargetRef};
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::{BodyData, BodyId, BodyIrReadTxn, BodyPath, BodyRef, ExprKind, PatData, ScopeId};

use super::{
    super::{PathCompletionNamespace, PathCompletionSite},
    sites::BodyScanSites,
};

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
        let sites = BodyScanSites::new(body);
        sites.walk_type_paths(Some(self.file_id), |site| {
            if let Some(completion_site) = self.site_for_type_path(body_ref, site.scope, site.path)
            {
                Self::remember_site(completion_site, site.path.source_span.len(), best);
            }
        });
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
            match &expr_data.kind {
                ExprKind::Path { path }
                | ExprKind::Record {
                    path: Some(path), ..
                } => {
                    self.scan_body_path(body_ref, expr_data.scope, path, best);
                }
                _ => {}
            }
        }

        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), |site| {
            self.scan_pat_data(body_ref, site.scope, site.data, best);
        });
    }

    /// Visits value paths directly owned by one pattern node.
    fn scan_pat_data(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        data: &PatData,
        best: &mut Option<(PathCompletionSite, u32)>,
    ) {
        if let Some(path) = data.kind.value_path() {
            self.scan_body_path(body_ref, scope, path, best);
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
        let qualifier = path.prefix_through(path.segment_count() - 1)?;

        // Expression and pattern paths can use modules and types as intermediate qualifiers, even
        // when the final completed path must eventually denote a value.
        Some(PathCompletionSite {
            body,
            scope,
            qualifier,
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
            let Some(qualifier) = path.prefix_through(idx - 1) else {
                continue;
            };

            Self::remember_site(
                PathCompletionSite {
                    body,
                    scope,
                    qualifier,
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
