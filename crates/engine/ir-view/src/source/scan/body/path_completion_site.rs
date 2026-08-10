//! Qualified paths and associated type binding sites over Body IR.
//!
//! The scanner keeps both interpretations of a path prefix: a DefMap-compatible module path and a
//! type-shaped qualifier that retains generic arguments or `<T as Trait>` anchors. It also
//! recognizes explicit associated bindings and the speculative pre-`=` form without conflating the
//! latter with ordinary generic-argument completion.
//!
//! ```text
//! let value: model::Us$0;    qualifier `model`, type context
//! Widget::<u8>::ne$0        type-shaped qualifier `Widget::<u8>`
//! Iterator<It$0 = u8>       explicit associated binding
//! Iterator<It$0>            possible binding overlaid on type completion
//! model::$0                  qualifier `model`, empty replacement span
//! ```

use rg_ir_model::{BodyRef, CrateRef, ScopeId};
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use rg_body_ir::{BodyIrReadTxn, BodyPath, BodyView, ExprKind, PatData, PatKind};

use super::super::{
    NarrowestSourceSite, TypeNamePosition,
    type_path::{AssociatedTypeBindingSyntax, TypePathCompletionSite},
};
use super::{
    BodyAssociatedTypeBindingSite, BodyQualifiedPathContext, PathCompletionSite,
    PatternCompletionKind, sites::BodyScanSites,
};

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
            // A body can end at the cursor while its last path is still being typed.
            if !body.source().span.touches(self.offset) {
                continue;
            }

            self.scan_type_paths(body_ref, body, &mut best);
            self.scan_body_paths(body_ref, body, &mut best);
        }

        Ok(best.finish())
    }

    /// Find a binding name that is already followed by `=`.
    ///
    /// In `Iterator<It$0 = u8>`, the `=` makes `It` unambiguously an associated type binding, so
    /// this site can replace ordinary type-argument completion.
    pub(crate) fn associated_type_binding_site_at(
        &self,
    ) -> Result<Option<BodyAssociatedTypeBindingSite>, PackageStoreError> {
        self.associated_type_binding_site_at_with(AssociatedTypeBindingSyntax::explicit_at)
    }

    /// Find a simple generic argument that may become a binding when the user types `=`.
    ///
    /// `Iterator<It$0>` is still valid ordinary type-argument syntax. Callers use this result as an
    /// overlay and retain the normal type candidates for `It`.
    pub(crate) fn implicit_associated_type_binding_site_at(
        &self,
    ) -> Result<Option<BodyAssociatedTypeBindingSite>, PackageStoreError> {
        self.associated_type_binding_site_at_with(AssociatedTypeBindingSyntax::implicit_at)
    }

    fn associated_type_binding_site_at_with(
        &self,
        site_at: impl Fn(&TypePath, u32) -> Option<AssociatedTypeBindingSyntax>,
    ) -> Result<Option<BodyAssociatedTypeBindingSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if !body.source().span.touches(self.offset) {
                continue;
            }

            BodyScanSites::new(body).walk_type_paths(Some(self.file_id), |site| {
                let Some(binding) = site_at(site.path, self.offset) else {
                    return;
                };
                best.consider(
                    BodyAssociatedTypeBindingSite {
                        body: body_ref,
                        scope: site.scope,
                        trait_ref: binding.trait_ref,
                        member_prefix_span: binding.member_prefix_span,
                        existing_bindings: binding.existing_bindings,
                    },
                    site.path.source_span.len(),
                );
            });
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
                    BodyQualifiedPathContext::Value,
                    best,
                ),
                ExprKind::Record {
                    path: Some(path), ..
                } => self.scan_body_path(
                    body_ref,
                    expr_data.scope,
                    path,
                    BodyQualifiedPathContext::Type,
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
        let (path, kind) = match &data.kind {
            PatKind::Record {
                path: Some(path), ..
            } => (path, PatternCompletionKind::RecordConstructor),
            PatKind::TupleStruct {
                path: Some(path), ..
            } => (path, PatternCompletionKind::TupleConstructor),
            PatKind::Path { path: Some(path) }
            | PatKind::Binding {
                binding: None,
                path: Some(path),
                ..
            } => (path, PatternCompletionKind::Name),
            PatKind::Binding { .. }
            | PatKind::Tuple { .. }
            | PatKind::Or { .. }
            | PatKind::Slice { .. }
            | PatKind::Ref { .. }
            | PatKind::Box { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported
            | PatKind::TupleStruct { path: None, .. }
            | PatKind::Record { path: None, .. }
            | PatKind::Path { path: None } => return,
        };
        self.scan_body_path(
            body_ref,
            scope,
            path,
            BodyQualifiedPathContext::Pattern(kind),
            best,
        );
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
            module_qualifier,
            associated_qualifier,
            member_prefix_span,
        } = TypePathCompletionSite::at(path, self.offset, position)?
        else {
            return None;
        };

        Some(PathCompletionSite {
            body,
            scope,
            module_qualifier,
            associated_qualifier,
            member_prefix_span,
            context: BodyQualifiedPathContext::Type,
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
        context: BodyQualifiedPathContext,
    ) -> Option<PathCompletionSite> {
        let last_segment_idx = path.segment_count().checked_sub(1)?;
        let last_segment_span = path.last_segment_span()?;
        let span = self.empty_member_span(path.source_span, last_segment_span)?;
        let module_qualifier = path.prefix_through(last_segment_idx);
        let associated_qualifier = path.associated_prefix_through(last_segment_idx)?.into();

        // Expression and pattern paths can use modules and types as intermediate qualifiers. The
        // surrounding syntax determines whether the missing final segment is a type or a value.
        Some(PathCompletionSite {
            body,
            scope,
            module_qualifier,
            associated_qualifier,
            member_prefix_span: span,
            context,
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
        context: BodyQualifiedPathContext,
        best: &mut NarrowestSourceSite<PathCompletionSite>,
    ) {
        for idx in 1..path.segment_count() {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if !span.touches(self.offset) {
                continue;
            }
            let module_qualifier = path.prefix_through(idx - 1);
            let Some(associated_qualifier) = path.associated_prefix_through(idx - 1) else {
                continue;
            };

            best.consider(
                PathCompletionSite {
                    body,
                    scope,
                    module_qualifier,
                    associated_qualifier: associated_qualifier.into(),
                    member_prefix_span: span,
                    context,
                },
                path.source_span.len(),
            );
        }

        if let Some(site) = self.empty_member_site_for_body_path(body, scope, path, context) {
            best.consider(site, path.source_span.len());
        }
    }
}
