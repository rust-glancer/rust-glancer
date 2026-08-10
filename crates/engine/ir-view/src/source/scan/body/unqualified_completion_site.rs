//! Unqualified completion site scanning over Body IR.
//!
//! This scanner recognizes single-segment type, value, and pattern paths and retains the lexical
//! cutoff that prevents later bindings from entering scope. Type paths inside body-local item
//! signatures keep that item's generic owner, and ambiguous identifier patterns keep the binding
//! whose inferred type can suggest enum variants. Request-local syntax can use the same scanner to
//! recover a scope when no ordinary Body IR path survived.
//!
//! ```text
//! let value: Us$0;       type names and visible generics
//! let value = inp$0;     value names before this source position
//! match value { Sta$0 }  constructors, with expected type when available
//! let _: [u8; LI$0];     request-local const syntax attached to a body scope
//! module::it$0;          not handled here: candidates come from `module`
//! ```

use rg_def_map::ItemSourceKind;
use rg_ir_model::{BindingId, BodyRef, CrateRef, GenericDefRef, ScopeId};
use rg_item_tree::TypePath;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_semantic_ir::ItemStore;

use rg_body_ir::{BodyIrReadTxn, BodyPath, BodyView, ExprKind, PatKind, StmtKind};

use super::super::{
    NarrowestSourceSite,
    type_path::{TypePathCompletionSite, identifier_prefix_at},
};
use super::sites::BodyScanSites;
use super::{BodyUnqualifiedNameContext, PatternCompletionKind, UnqualifiedCompletionSite};

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
            if !body.source().span.touches(self.offset) {
                continue;
            }

            let item_store = self.body_ir.body_item_store(body_ref)?;
            self.scan_type_names(body_ref, body, item_store, &mut best);
            self.scan_value_names(body_ref, body, &mut best);
            self.scan_pattern_names(body_ref, body, &mut best);
        }

        Ok(best.finish())
    }

    /// Attaches request-local identifier syntax to the nearest indexed body scope.
    ///
    /// Some domains, notably const expressions, retain their source range outside Body IR's
    /// ordinary path walkers. The syntax classifier owns the spelling while this method supplies
    /// only lexical scope, generic ownership, and source-order visibility.
    ///
    /// `body_owner_start` is used for speculative syntax whose recovered body ends before the
    /// cursor. Matching the declaration's start reconnects that spelling to the original body
    /// without treating every body in the file as a possible owner.
    pub(crate) fn site_at_syntax_name(
        &self,
        context: BodyUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
        body_owner_start: Option<u32>,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if !body.source().span.touches(self.offset)
                && body_owner_start
                    .is_none_or(|owner_start| body.source().span.text.start != owner_start)
            {
                continue;
            }

            let mut scope = body.param_scope();
            let mut visible_bindings = Self::binding_cutoff_at(body, self.offset);
            let mut source_len = body.source().span.len();

            // A real enclosing expression is the strongest scope fact for empty call arguments
            // and similar recovered children. Broad block expressions use their inner scope but
            // derive the binding cutoff from source order so earlier statements remain visible.
            for expr in body.exprs() {
                if !expr.source.is_written_in_file(self.file_id)
                    || !expr.source.span.touches(self.offset)
                    || expr.source.span.len() > source_len
                {
                    continue;
                }
                scope = match &expr.kind {
                    ExprKind::Block { scope, .. } | ExprKind::Closure { scope, .. } => *scope,
                    _ => expr.scope,
                };
                visible_bindings = if matches!(&expr.kind, ExprKind::Block { .. }) {
                    Self::binding_cutoff_at(body, self.offset)
                } else {
                    expr.visible_bindings
                };
                source_len = expr.source.span.len();
            }

            // Missing let annotations and initializers may have no expression node. The statement
            // still preserves their lexical scope and the bindings introduced by that let, which
            // must not be visible within its own initializer.
            for statement in body.statements() {
                if !statement.source.is_written_in_file(self.file_id)
                    || !statement.source.span.touches(self.offset)
                    || statement.source.span.len() > source_len
                {
                    continue;
                }
                if let StmtKind::Let {
                    scope: statement_scope,
                    bindings,
                    ..
                } = &statement.kind
                {
                    scope = *statement_scope;
                    visible_bindings = bindings
                        .iter()
                        .map(|binding| binding.0)
                        .min()
                        .unwrap_or_else(|| Self::binding_cutoff_at(body, self.offset));
                    source_len = statement.source.span.len();
                }
            }

            let generic_owner = self.empty_site_generic_owner(body_ref, body)?;
            best.consider(
                UnqualifiedCompletionSite {
                    body: body_ref,
                    scope,
                    member_prefix_span,
                    member_prefix: member_prefix.clone(),
                    context,
                    generic_owner,
                    expected_type_binding: None,
                    visible_bindings,
                },
                body.source().span.len(),
            );
        }

        Ok(best.finish())
    }

    fn binding_cutoff_at(body: BodyView<'_>, offset: u32) -> usize {
        body.bindings()
            .iter()
            .position(|binding| binding.source.span.text.start >= offset)
            .unwrap_or_else(|| body.bindings().len())
    }

    /// Finds a body-local declaration that owns the empty type position, if any.
    fn empty_site_generic_owner(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
    ) -> Result<Option<GenericDefRef>, PackageStoreError> {
        let Some(item_store) = self.body_ir.body_item_store(body_ref)? else {
            return Ok(None);
        };
        let mut best = NarrowestSourceSite::new();
        for item in item_store.semantic_items() {
            let ItemSourceKind::Body(source) = item.source().kind else {
                continue;
            };
            if source.body != body_ref {
                continue;
            }
            let Some(source_item) = body.source_item(source.item) else {
                continue;
            };
            if source_item.file_id == self.file_id && source_item.span.touches(self.offset) {
                best.consider(GenericDefRef::from(item.item()), source_item.span.len());
            }
        }
        Ok(best.finish())
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_names(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        item_store: Option<&ItemStore>,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_type_paths(Some(self.file_id), |site| {
            if let Some(completion_site) = self.site_for_type_path(
                body_ref,
                site.scope,
                site.visible_bindings,
                Self::generic_owner(item_store, body_ref, site.owner_item),
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
                        BodyUnqualifiedNameContext::Value,
                        None,
                        best,
                    );
                }
                _ => {}
            }
        }
    }

    /// Scans every pattern source owned by the body, including function and closure parameters.
    ///
    /// An unresolved identifier pattern is lowered as a binding. Keeping that binding id lets
    /// candidate assembly reuse its already-inferred expected type for enum-variant suggestions.
    fn scan_pattern_names(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
    ) {
        let visible_bindings = body.bindings().len();
        BodyScanSites::new(body).walk_pats(Some(self.file_id), Some(self.offset), |site| {
            let (path, kind, expected_type_binding) = match &site.data.kind {
                PatKind::Record { path, .. } => (
                    path.as_ref(),
                    PatternCompletionKind::RecordConstructor,
                    None,
                ),
                PatKind::TupleStruct { path, .. } => {
                    (path.as_ref(), PatternCompletionKind::TupleConstructor, None)
                }
                PatKind::Path { path } => (path.as_ref(), PatternCompletionKind::Name, None),
                PatKind::Binding { binding, path, .. } => {
                    (path.as_ref(), PatternCompletionKind::Name, *binding)
                }
                PatKind::Tuple { .. }
                | PatKind::Or { .. }
                | PatKind::Slice { .. }
                | PatKind::Ref { .. }
                | PatKind::Box { .. }
                | PatKind::Rest
                | PatKind::Literal { .. }
                | PatKind::Range { .. }
                | PatKind::ConstBlock { .. }
                | PatKind::Wildcard
                | PatKind::Unsupported => return,
            };

            if let Some(path) = path {
                self.scan_body_path(
                    body_ref,
                    site.scope,
                    visible_bindings,
                    path,
                    BodyUnqualifiedNameContext::Pattern(kind),
                    expected_type_binding,
                    best,
                );
                return;
            }

            let Some(binding) = expected_type_binding else {
                return;
            };
            let Some(binding_data) = body.binding(binding) else {
                return;
            };
            let Some(span) = binding_data.name_span else {
                return;
            };
            if !span.touches(self.offset) {
                return;
            }

            best.consider(
                UnqualifiedCompletionSite {
                    body: body_ref,
                    scope: site.scope,
                    member_prefix_span: span,
                    member_prefix: identifier_prefix_at(
                        binding_data.name.as_deref().unwrap_or_default(),
                        span,
                        self.offset,
                    ),
                    context: BodyUnqualifiedNameContext::Pattern(kind),
                    generic_owner: None,
                    expected_type_binding: Some(binding),
                    visible_bindings,
                },
                site.data.source.span.len(),
            );
        });
    }

    fn site_for_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        generic_owner: Option<GenericDefRef>,
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
            generic_owner,
            expected_type_binding: None,
            visible_bindings,
        })
    }

    /// Maps syntax retained by the parent body back to the declaration that owns its generics.
    ///
    /// The lookup only runs after a type path touches the cursor. Body-local item stores are
    /// deliberately small and file-sharded, so a linear scan avoids a retained reverse index used
    /// solely by this point query.
    fn generic_owner(
        item_store: Option<&ItemStore>,
        body: BodyRef,
        owner_item: Option<rg_item_tree::ItemTreeId>,
    ) -> Option<GenericDefRef> {
        let owner_item = owner_item?;
        item_store?
            .semantic_items()
            .find(|item| {
                matches!(
                    item.source().kind,
                    ItemSourceKind::Body(source)
                        if source.body == body && source.item == owner_item
                )
            })
            .map(|item| item.item().into())
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &BodyPath,
        context: BodyUnqualifiedNameContext,
        expected_type_binding: Option<BindingId>,
        best: &mut NarrowestSourceSite<UnqualifiedCompletionSite>,
    ) {
        let Some(def_map_path) = path.as_def_map_path() else {
            return;
        };
        if !def_map_path.is_relative() || path.segment_count() != 1 {
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
                context,
                generic_owner: None,
                expected_type_binding,
                visible_bindings,
            },
            path.source_span.len(),
        );
    }
}
