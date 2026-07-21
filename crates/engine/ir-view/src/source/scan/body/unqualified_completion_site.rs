//! Unqualified completion site scanning over Body IR.
//!
//! This scanner recognizes single-segment paths such as `inp$0` and `Us$0`.
//! Qualified paths are left to the path-completion scanner because their
//! candidate set comes from the resolved qualifier rather than lexical scope.
//!
//! ```text
//! let value: Us$0; type names from the surrounding lexical/module scopes
//! let value = inp$0; value names, limited to bindings declared before this expression
//! module::it$0; not handled here: candidates come from `module`
//! ```

use rg_ir_model::{BodyRef, CrateRef, ScopeId};
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use rg_body_ir::{BodyIrReadTxn, BodyPath, BodyView, ExprKind};

use super::super::{
    NarrowestSourceSite,
    type_path::{TypePathCompletionSite, identifier_prefix_at},
};
use super::sites::BodyScanSites;
use super::{BodyUnqualifiedNameContext, UnqualifiedCompletionSite};

/// Finds the source site that belongs to an unqualified completion offset.
///
/// The site retains the lexical scope and source-order binding cutoff. Lowering has already built
/// the complete binding arena, but names declared after the cursor must not appear as candidates.
pub(crate) struct UnqualifiedCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> UnqualifiedCompletionSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Returns the smallest type or value name prefix that accepts completions.
    pub(crate) fn site_at_name(
        &self,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if !body.source().span.contains(self.offset) {
                continue;
            }

            self.scan_type_names(body_ref, body, &mut best);
            self.scan_value_names(body_ref, body, &mut best);
        }

        Ok(best.finish())
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_names(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_type_paths(Some(self.file_id), |site| {
            if let Some(completion_site) = self.site_for_type_path(
                body_ref,
                site.scope,
                site.visible_bindings,
                site.path,
                site.position,
            ) {
                best.consider(completion_site, site.path.source_span.len());
            }
        });
    }

    /// Scans expression paths where value-namespace completions can appear.
    fn scan_value_names(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
    ) {
        for expr_data in body.exprs() {
            if !expr_data.source.is_written_in_file(self.file_id) {
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
        position: super::super::TypeNamePosition,
    ) -> Option<UnqualifiedCompletionSite> {
        // This scanner owns only complete unqualified paths. The first segment of a longer path
        // needs qualified-path recovery policy before it can safely use lexical candidates.
        if path.segments.len() != 1 {
            return None;
        }
        let TypePathCompletionSite::Unqualified {
            member_prefix_span,
            member_prefix,
            position,
        } = TypePathCompletionSite::at(path, self.offset, position)?
        else {
            return None;
        };

        Some(UnqualifiedCompletionSite {
            body,
            scope,
            member_prefix_span,
            member_prefix,
            context: BodyUnqualifiedNameContext::Type(position),
            visible_bindings,
        })
    }

    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &BodyPath,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
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

        best.consider(
            UnqualifiedCompletionSite {
                body,
                scope,
                member_prefix_span: span,
                member_prefix: identifier_prefix_at(
                    def_map_path.single_name().unwrap_or_default(),
                    span,
                    self.offset,
                ),
                context: BodyUnqualifiedNameContext::Value,
                visible_bindings,
            },
            path.source_span.len(),
        );
    }
}
