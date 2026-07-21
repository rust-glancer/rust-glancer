//! Qualified-path completion site scanning over Body IR.
//!
//! Path completion scans recognize partially typed segments in paths such as
//! `crate::module::Us` and return the qualifier, replacement span, and expected namespace.
//!
//! ```text
//! let value: model::Us$0;  qualifier `model`, complete in the type namespace
//! model::make$0()          qualifier `model`, complete in the value namespace
//! model::$0                qualifier `model`, use an empty replacement span
//! ```

use rg_ir_model::{BodyRef, CrateRef, ScopeId};
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use rg_body_ir::{BodyIrReadTxn, BodyPath, BodyView, ExprKind, PatData};

use super::super::{NarrowestSourceSite, TypeNamePosition, type_path::TypePathCompletionSite};
use super::{PathCompletionSite, sites::BodyScanSites};
use crate::lookup::name::ValueOrTypeNamespace;

/// Finds the source site that belongs to a qualified-path completion offset.
///
/// The surrounding syntax supplies the expected namespace: type annotations and record
/// constructors request types, while ordinary expression and tuple/unit pattern paths request
/// values.
pub(crate) struct PathCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> PathCompletionSiteScanner<'txn, 'db> {
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

    /// Returns the smallest type or value path whose segment prefix accepts completions.
    pub(crate) fn site_at_path(&self) -> Result<Option<PathCompletionSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            // Body spans are a cheap first filter before scanning every expression and statement.
            if !body.source().span.contains(self.offset) {
                continue;
            }

            self.scan_type_paths(body_ref, body, &mut best);
            self.scan_body_paths(body_ref, body, &mut best);
        }

        Ok(best.finish())
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_paths(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut NarrowestSourceSite<PathCompletionSite>,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_type_paths(Some(self.file_id), |site| {
            if let Some(completion_site) =
                self.site_for_type_path(body_ref, site.scope, site.path, site.position)
            {
                best.consider(completion_site, site.path.source_span.len());
            }
        });
    }

    /// Scans expression and pattern paths, preserving the namespace selected by their syntax.
    fn scan_body_paths(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut NarrowestSourceSite<PathCompletionSite>,
    ) {
        for expr_data in body.exprs() {
            if !expr_data.source.is_written_in_file(self.file_id) {
                continue;
            }
            match &expr_data.kind {
                ExprKind::Path { path } => self.scan_body_path(
                    body_ref,
                    expr_data.scope,
                    path,
                    ValueOrTypeNamespace::Values,
                    best,
                ),
                ExprKind::Record {
                    path: Some(path), ..
                } => self.scan_body_path(
                    body_ref,
                    expr_data.scope,
                    path,
                    ValueOrTypeNamespace::Types,
                    best,
                ),
                _ => {}
            }
        }

        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), |site| {
            self.scan_pat_data(body_ref, site.scope, site.data, best);
        });
    }

    /// Visits constructor paths directly owned by one pattern node.
    fn scan_pat_data(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        data: &PatData,
        best: &mut NarrowestSourceSite<PathCompletionSite>,
    ) {
        if let Some(path) = data.kind.record_path() {
            self.scan_body_path(body_ref, scope, path, ValueOrTypeNamespace::Types, best);
        } else if let Some(path) = data.kind.value_path() {
            self.scan_body_path(body_ref, scope, path, ValueOrTypeNamespace::Values, best);
        }
    }

    /// Finds a partially typed type path segment after at least one qualifier segment.
    ///
    /// For `outer::inner::Ty$0`, the returned qualifier is `outer::inner` and the replacement span
    /// covers `Ty`.
    fn site_for_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &TypePath,
        position: TypeNamePosition,
    ) -> Option<PathCompletionSite> {
        let TypePathCompletionSite::Qualified {
            qualifier,
            member_prefix_span,
        } = TypePathCompletionSite::at(path, self.offset, position)?
        else {
            return None;
        };

        Some(PathCompletionSite {
            body,
            scope,
            qualifier,
            member_prefix_span,
            namespace: ValueOrTypeNamespace::Types,
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
        namespace: ValueOrTypeNamespace,
    ) -> Option<PathCompletionSite> {
        let last_segment_span = path.segment_span(path.segment_count().checked_sub(1)?)?;
        let span = self.empty_member_span(path.source_span, last_segment_span)?;
        let qualifier = path.prefix_through(path.segment_count() - 1)?;

        // Expression and pattern paths can use modules and types as intermediate qualifiers. The
        // surrounding syntax determines whether the missing final segment is a type or a value.
        Some(PathCompletionSite {
            body,
            scope,
            qualifier,
            member_prefix_span: span,
            namespace,
        })
    }

    /// Finds a partially typed constructor path segment after at least one qualifier segment.
    ///
    /// A record path such as `model::Us$0 { ... }` selects the type namespace; a call-like path such
    /// as `model::ma$0ke()` selects the value namespace.
    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &BodyPath,
        namespace: ValueOrTypeNamespace,
        best: &mut NarrowestSourceSite<PathCompletionSite>,
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

            best.consider(
                PathCompletionSite {
                    body,
                    scope,
                    qualifier,
                    member_prefix_span: span,
                    namespace,
                },
                path.source_span.len(),
            );
        }

        if let Some(site) = self.empty_member_site_for_body_path(body, scope, path, namespace) {
            best.consider(site, path.source_span.len());
        }
    }
}
