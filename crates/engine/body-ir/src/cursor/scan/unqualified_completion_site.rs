//! Unqualified completion site scanning.
//!
//! This scanner recognizes single-segment paths such as `inp$0` and `Us$0`.
//! Qualified paths are left to the path-completion scanner because their
//! candidate set comes from the resolved qualifier rather than lexical scope.

use rg_def_map::TargetRef;
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{
    BodyData, BodyId, BodyIrReadTxn, BodyPath, BodyRef, ExprKind, ScopeId,
    cursor::{UnqualifiedCompletionNamespace, UnqualifiedCompletionSite},
};

use super::sites::BodyScanSites;

/// Finds the source site that belongs to an unqualified completion offset.
pub(crate) struct UnqualifiedCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> UnqualifiedCompletionSiteScanner<'txn, 'db> {
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

    /// Returns the smallest type or value name prefix that accepts completions.
    pub(crate) fn site_at_name(
        &self,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(UnqualifiedCompletionSite, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            self.scan_type_names(body_ref, body, &mut best);
            self.scan_value_names(body_ref, body, &mut best);
        }

        Ok(best.map(|(site, _)| site))
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_names(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_type_paths(Some(self.file_id), |site| {
            if let Some(completion_site) =
                self.site_for_type_path(body_ref, site.scope, site.visible_bindings, site.path)
            {
                Self::remember_site(completion_site, site.path.source_span.len(), best);
            }
        });
    }

    /// Scans expression paths where value-namespace completions can appear.
    fn scan_value_names(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
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
                    self.scan_body_path(
                        body_ref,
                        expr_data.scope,
                        expr_data.visible_bindings,
                        path,
                        best,
                    );
                }
                _ => {}
            }
        }
    }

    fn site_for_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &TypePath,
    ) -> Option<UnqualifiedCompletionSite> {
        if path.absolute || path.segments.len() != 1 {
            return None;
        }
        let segment = path.segments.first()?;
        if !segment.span.touches(self.offset) {
            return None;
        }

        Some(UnqualifiedCompletionSite {
            body,
            scope,
            member_prefix_span: segment.span,
            namespace: UnqualifiedCompletionNamespace::Types,
            visible_bindings,
        })
    }

    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &BodyPath,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        let Some(def_map_path) = path.as_def_map_path() else {
            return;
        };
        if def_map_path.absolute || path.segment_count() != 1 {
            return;
        }
        let Some(span) = path.segment_span(0) else {
            return;
        };
        if !span.touches(self.offset) {
            return;
        }

        Self::remember_site(
            UnqualifiedCompletionSite {
                body,
                scope,
                member_prefix_span: span,
                namespace: UnqualifiedCompletionNamespace::Values,
                visible_bindings,
            },
            path.source_span.len(),
            best,
        );
    }

    /// Keeps nested path behavior predictable by choosing the shortest matching path syntax.
    fn remember_site(
        site: UnqualifiedCompletionSite,
        source_len: u32,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        if best
            .as_ref()
            .is_none_or(|(_, best_len)| source_len < *best_len)
        {
            *best = Some((site, source_len));
        }
    }
}
