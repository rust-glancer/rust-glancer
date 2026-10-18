//! Point-query scanning for source requests at one offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.
//!
//! ```text
//! items.push$0(value)
//!       ^^^^ select the method name, not the enclosing call or block
//!
//! Action::Sta$0rt
//!         ^^^^^ keep a candidate for this path segment as well
//! ```
//!
//! The first candidate gives editor queries the smallest AST-like body node. Path scanners add the
//! second lane because one lowered expression can contain several independently navigable path
//! segments.

use rg_body_ir::{BodyIrReadTxn, BodyOwner, BodyView, ExprData, ExprKind, PatKind};
use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    BindingId, BodyRef, CrateRef, DefMapRef, EnumVariantRef, ExprId, FieldRef, FileId,
    SemanticItemRef, Span, TypeDefId,
};
use rg_package_store::PackageStoreError;

use super::{
    BindingSurface, BodySourceCandidate, RecordFieldKeySurface,
    paths::{BodyPathSourceScanner, TypePathSourceScanner},
    record_pat_shorthand::RecordPatShorthandBinding,
    sites::BodyScanSites,
};
use crate::source::scan::NarrowestSourceSite;

/// Scans one Body IR transaction for all source interpretations at a cursor offset.
///
/// Ordinary source selects one narrowest body-local candidate plus any type/value path segments
/// that touch the same offset. If a macro copied the selected token into several nodes, keep each
/// expanded use. Analysis can then interpret all uses without Body IR choosing a navigation target.
pub(crate) struct BodyCursorScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
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

    /// Returns body-local candidates that can answer an editor query at this exact offset.
    pub(crate) fn scan(&self) -> Result<Vec<BodySourceCandidate>, PackageStoreError> {
        let Some(body_ref) = self.body_at()? else {
            return Ok(Vec::new());
        };
        let Some(body) = self.body_ir.body(body_ref)? else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        candidates.extend(self.candidate_at_body(body_ref, body)?);
        TypePathSourceScanner::at(body_ref, body, self.file_id, self.offset, &mut candidates)
            .scan();
        BodyPathSourceScanner::at(body_ref, body, self.file_id, self.offset, &mut candidates)
            .scan();

        Ok(candidates)
    }

    /// Finds the innermost enclosing body at the cursor offset.
    fn body_at(&self) -> Result<Option<BodyRef>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if !body.source().span.contains(self.offset) {
                continue;
            }

            let body_len = body.source().span.len();
            best.consider(body_ref, body_len);
        }

        Ok(best.finish())
    }

    /// Pick the narrowest source span, retaining expanded uses that share that span.
    /// Outside macro calls, the body itself is the fallback when no smaller candidate matches.
    fn candidate_at_body(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
    ) -> Result<Vec<BodySourceCandidate>, PackageStoreError> {
        let mut best = BestCursorCandidate::new(BodySourceCandidate::Body {
            body: body_ref,
            span: body.source().span,
        });
        let record_shorthand_values = BodyScanSites::new(body).record_expr_shorthand_value_ids();
        let record_shorthand_bindings =
            RecordPatShorthandBinding::collect(body, Some(self.file_id), Some(self.offset));

        // `body_at` chooses the innermost enclosing body. When the cursor is on a nested
        // fn/const/static declaration name, that body owns the initializer/block, while the
        // declaration item still lives in the parent body-local item store.
        self.consider_body_owner_declaration(body, &mut best)?;

        // Macro expansion can map many generated nodes back to the invocation span. Give the
        // written macro path a precise candidate before scanning the arguments. Helper invocations
        // created by a template have no written name token and stay hidden.
        self.consider_macro_calls(body_ref, body, &mut best);

        // Inside an expansion, a broad handwritten parent (such as the function block) must
        // not mask a mapped argument or answer for a generated helper. Precise argument tokens
        // are considered below, while ordinary parent expressions stay outside this boundary.
        let inside_expansion = self.macro_source_touches_offset(body);

        // First look at expressions under the cursor: calls, paths, field accesses, literals, and
        // so on. Record shorthand values are skipped here because the key token has its own
        // source-level candidate.
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            if expr.source.file_id != self.file_id {
                continue;
            }
            if !expr.source.is_written() {
                self.consider_record_expr_fields(body_ref, body, expr, &mut best);
                let span = Self::expression_reference_span(expr).unwrap_or(expr.source.span);
                if span.touches(self.offset) && body.source_map().is_macro_argument(span) {
                    best.consider_mapped(
                        span,
                        BodySourceCandidate::Expr {
                            body: body_ref,
                            expr: ExprId(expr_idx),
                            span,
                        },
                    );
                }
                continue;
            }
            if inside_expansion {
                continue;
            }
            if expr.source.span.touches(self.offset) {
                if record_shorthand_values.contains(&ExprId(expr_idx)) {
                    continue;
                }
                // Record expression keys can resolve to fields, while the record expression itself
                // is still an ordinary expression candidate.
                self.consider_record_expr_fields(body_ref, body, expr, &mut best);
                let span = Self::expression_reference_span(expr)
                    .filter(|span| span.touches(self.offset))
                    .unwrap_or(expr.source.span);
                best.consider(
                    span,
                    BodySourceCandidate::Expr {
                        body: body_ref,
                        expr: ExprId(expr_idx),
                        span,
                    },
                );
            }
        }

        // Record pattern keys are not expressions, so check them separately before ordinary
        // binding declarations.
        self.consider_record_pat_fields(body_ref, body, &mut best);

        // Then look for local bindings introduced by params, lets, closures, and patterns. For
        // shorthand record patterns, keep enough surface information for rename to expand the field.
        for (binding_idx, binding) in body.bindings().iter().enumerate() {
            let binding_span = binding.name_span.unwrap_or(binding.source.span);
            if binding.source.file_id != self.file_id
                || !body.contains_source_span(binding.source, binding_span)
            {
                continue;
            }
            if binding_span.touches(self.offset) {
                let binding_id = BindingId(binding_idx);
                let surface = if let Some(shorthand) = record_shorthand_bindings
                    .iter()
                    .find(|shorthand| shorthand.binding == binding_id)
                {
                    BindingSurface::RecordPatShorthand {
                        key: shorthand.key.clone(),
                        field_span: shorthand.field_span,
                        pat_span: shorthand.pat_span,
                        binding_name_span: shorthand.binding_name_span,
                    }
                } else {
                    BindingSurface::Plain
                };
                best.consider_mapped(
                    binding_span,
                    BodySourceCandidate::Binding {
                        body: body_ref,
                        binding: binding_id,
                        span: binding_span,
                        surface,
                    },
                );
            }
        }

        if let Some(item_store) = self.body_ir.body_item_store(body_ref)? {
            // Body-local items live in the same source range as the body but are stored in a local
            // item store. Their names can be the best answer when the cursor is on a nested item
            // declaration or one of its fields/variants.
            for item in item_store.semantic_items() {
                if let ItemSourceKind::Body(source) = item.source().kind
                    && source.body == body_ref
                    && !body.source_item_is_written(source.item)
                {
                    continue;
                }
                if item.source().file_id != self.file_id {
                    continue;
                }
                let declaration_span = match item.source().kind {
                    ItemSourceKind::Body(source) if source.body == body_ref => body
                        .source_item(source.item)
                        .and_then(|item| item.name_span)
                        .unwrap_or_else(|| item.span().unwrap_or(body.source().span)),
                    _ => item.span().unwrap_or(body.source().span),
                };
                if declaration_span.touches(self.offset) {
                    match item.item() {
                        SemanticItemRef::TypeDef(_)
                        | SemanticItemRef::Trait(_)
                        | SemanticItemRef::TypeAlias(_) => best.consider(
                            declaration_span,
                            BodySourceCandidate::LocalItem {
                                item: item.item(),
                                span: declaration_span,
                            },
                        ),
                        SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => best.consider(
                            declaration_span,
                            BodySourceCandidate::LocalValueItem {
                                item: item.item(),
                                span: declaration_span,
                            },
                        ),
                        SemanticItemRef::Function(function) => best.consider(
                            declaration_span,
                            BodySourceCandidate::LocalFunction {
                                function,
                                span: declaration_span,
                            },
                        ),
                        SemanticItemRef::Impl(_) => {}
                    }
                }

                if let SemanticItemRef::TypeDef(ty) = item.item() {
                    self.consider_fields(item_store, ty, &mut best);
                    self.consider_variants(item_store, ty, &mut best);
                }
            }
        }

        if inside_expansion && best.candidates.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(best.finish())
        }
    }

    /// Projects a nested body back to its body-local item declaration when the cursor is on the
    /// owner name, so editor queries target the function/const/static item instead of the body.
    fn consider_body_owner_declaration(
        &self,
        body: BodyView<'_>,
        best: &mut BestCursorCandidate,
    ) -> Result<(), PackageStoreError> {
        let Some((item, declaration_span)) = self.body_owner_declaration(body)? else {
            return Ok(());
        };
        if !declaration_span.touches(self.offset) {
            return Ok(());
        }

        match item {
            SemanticItemRef::Function(function) => best.consider(
                declaration_span,
                BodySourceCandidate::LocalFunction {
                    function,
                    span: declaration_span,
                },
            ),
            SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => best.consider(
                declaration_span,
                BodySourceCandidate::LocalValueItem {
                    item,
                    span: declaration_span,
                },
            ),
            SemanticItemRef::TypeDef(_)
            | SemanticItemRef::Trait(_)
            | SemanticItemRef::Impl(_)
            | SemanticItemRef::TypeAlias(_) => {}
        }

        Ok(())
    }

    fn consider_macro_calls(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut BestCursorCandidate,
    ) {
        for call in body.macro_calls() {
            if call.source.file_id != self.file_id
                || !(call.source.is_written()
                    || body.source_map().is_macro_argument(call.name_span))
            {
                continue;
            }
            if !call.name_span.touches(self.offset) {
                continue;
            }

            best.consider(
                call.name_span,
                BodySourceCandidate::MacroCall {
                    body: body_ref,
                    definition: call.definition,
                    file_id: call.source.file_id,
                    span: call.name_span,
                },
            );
        }
    }

    fn body_owner_declaration(
        &self,
        body: BodyView<'_>,
    ) -> Result<Option<(SemanticItemRef, Span)>, PackageStoreError> {
        match body.owner() {
            BodyOwner::Function(function) => {
                if let DefMapRef::Body(parent_body_ref) = function.origin
                    && let Some(item_store) = self.body_ir.body_item_store(parent_body_ref)?
                    && let Some(data) = item_store.function_data(function.id)
                {
                    Ok(data
                        .name_span
                        .map(|span| (SemanticItemRef::from(function), span)))
                } else {
                    Ok(None)
                }
            }
            BodyOwner::Const(konst) => {
                if let DefMapRef::Body(parent_body_ref) = konst.origin
                    && let Some(item_store) = self.body_ir.body_item_store(parent_body_ref)?
                    && let Some(data) = item_store.const_data(konst.id)
                {
                    Ok(data
                        .name_span
                        .map(|span| (SemanticItemRef::from(konst), span)))
                } else {
                    Ok(None)
                }
            }
            BodyOwner::Static(static_ref) => {
                if let DefMapRef::Body(parent_body_ref) = static_ref.origin
                    && let Some(item_store) = self.body_ir.body_item_store(parent_body_ref)?
                    && let Some(data) = item_store.static_data(static_ref.id)
                {
                    Ok(data
                        .name_span
                        .map(|span| (SemanticItemRef::from(static_ref), span)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Recognize macro ranges even when they produce no expression, such as type expansions.
    fn macro_source_touches_offset(&self, body: BodyView<'_>) -> bool {
        body.macro_calls().iter().any(|call| {
            call.source.file_id == self.file_id && call.source.span.touches(self.offset)
        }) || body.exprs().iter().any(|expr| {
            !expr.source.is_written()
                && expr.source.file_id == self.file_id
                && expr.source.span.touches(self.offset)
        }) || body.pats().iter().any(|pat| {
            !pat.source.is_written()
                && pat.source.file_id == self.file_id
                && pat.source.span.touches(self.offset)
        }) || body.bindings().iter().any(|binding| {
            !binding.source.is_written()
                && binding.source.file_id == self.file_id
                && binding.source.span.touches(self.offset)
        })
    }

    fn consider_record_expr_fields(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        expr: &ExprData,
        best: &mut BestCursorCandidate,
    ) {
        let ExprKind::Record {
            path: Some(owner),
            fields,
            ..
        } = &expr.kind
        else {
            return;
        };
        let Some(owner) = owner.as_def_map_path() else {
            return;
        };

        for field in fields {
            if !field.syntax.is_explicit()
                || !field.key_span.touches(self.offset)
                || !body.contains_source_span(expr.source, field.key_span)
            {
                continue;
            }
            best.consider_mapped(
                field.key_span,
                BodySourceCandidate::RecordFieldKey {
                    body: body_ref,
                    scope: expr.scope,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: expr.source.file_id,
                    span: field.key_span,
                    surface: RecordFieldKeySurface::Explicit,
                },
            );
        }
    }

    fn consider_record_pat_fields(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut BestCursorCandidate,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), true, |site| {
            let PatKind::Record {
                path: Some(owner),
                fields,
                ..
            } = &site.data.kind
            else {
                return;
            };
            let Some(owner) = owner.as_def_map_path() else {
                return;
            };

            for field in fields {
                if !field.syntax.is_explicit()
                    || !field.key_span.touches(self.offset)
                    || !body.contains_source_span(site.data.source, field.key_span)
                {
                    continue;
                }
                best.consider_mapped(
                    field.key_span,
                    BodySourceCandidate::RecordFieldKey {
                        body: body_ref,
                        scope: site.scope,
                        owner: owner.clone(),
                        key: field.key.clone(),
                        file_id: site.data.source.file_id,
                        span: field.key_span,
                        surface: RecordFieldKeySurface::Explicit,
                    },
                );
            }
        });
    }

    fn expression_reference_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::Path { path } => path.last_segment_span(),
            ExprKind::MethodCall {
                method_name_span, ..
            } => *method_name_span,
            ExprKind::Field { field_span, .. } => *field_span,
            ExprKind::Record {
                path: Some(path), ..
            } => path.last_segment_span(),
            _ => None,
        }
    }

    fn consider_fields(
        &self,
        item_store: &rg_semantic_ir::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        best: &mut BestCursorCandidate,
    ) {
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = item_store.struct_data(id) else {
                    return;
                };
                if data.source.file_id != self.file_id {
                    return;
                }
                for (index, field) in data.fields.fields().iter().enumerate() {
                    if field.span.touches(self.offset) {
                        best.consider(
                            field.span,
                            BodySourceCandidate::LocalField {
                                field: FieldRef { owner: ty, index },
                                span: field.span,
                            },
                        );
                    }
                }
            }
            TypeDefId::Union(id) => {
                let Some(data) = item_store.union_data(id) else {
                    return;
                };
                if data.source.file_id != self.file_id {
                    return;
                }
                for (index, field) in data.fields.iter().enumerate() {
                    if field.span.touches(self.offset) {
                        best.consider(
                            field.span,
                            BodySourceCandidate::LocalField {
                                field: FieldRef { owner: ty, index },
                                span: field.span,
                            },
                        );
                    }
                }
            }
            TypeDefId::Enum(_) => {}
        }
    }

    fn consider_variants(
        &self,
        item_store: &rg_semantic_ir::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        best: &mut BestCursorCandidate,
    ) {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return;
        };
        let Some(data) = item_store.enum_data(enum_id) else {
            return;
        };
        if data.source.file_id != self.file_id {
            return;
        }
        for (index, variant) in data.variants.iter().enumerate() {
            if variant.name_span.touches(self.offset) {
                best.consider(
                    variant.name_span,
                    BodySourceCandidate::LocalEnumVariant {
                        variant: EnumVariantRef {
                            origin: ty.origin,
                            enum_id,
                            index,
                        },
                        span: variant.name_span,
                    },
                );
            }
        }
    }
}

/// Tracks the narrowest body-local candidate seen while scanning one body.
///
/// For `both!(run)` expanding to `(First.run(), Second.run())`, both method calls have the same
/// source span. Keep both candidates so a cursor query can report both meanings of `run`.
struct BestCursorCandidate {
    candidates: Vec<BodySourceCandidate>,
    fallback: BodySourceCandidate,
}

impl BestCursorCandidate {
    fn new(fallback: BodySourceCandidate) -> Self {
        Self {
            candidates: Vec::new(),
            fallback,
        }
    }

    fn consider(&mut self, span: Span, candidate: BodySourceCandidate) {
        if self
            .candidates
            .first()
            .is_none_or(|best| span.len() < best.span().len())
        {
            self.candidates.clear();
            self.candidates.push(candidate);
        }
    }

    /// Keep distinct uses of the same original span; a narrower span still replaces them all.
    fn consider_mapped(&mut self, span: Span, candidate: BodySourceCandidate) {
        if self
            .candidates
            .first()
            .is_some_and(|best| best.span() == span)
        {
            if !self.candidates.contains(&candidate) {
                self.candidates.push(candidate);
            }
        } else {
            self.consider(span, candidate);
        }
    }

    fn finish(self) -> Vec<BodySourceCandidate> {
        if self.candidates.is_empty() {
            vec![self.fallback]
        } else {
            self.candidates
        }
    }
}
