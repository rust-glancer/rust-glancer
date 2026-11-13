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

use rg_body_ir::{BodyOwner, BodyReader, BodyView, ExprData, ExprKind, PatKind};
use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    BindingId, BodyRef, CrateRef, DefMapRef, EnumVariantRef, ExprId, FieldRef, FileId,
    SemanticItemRef, Span, TypeDefId,
};
use rg_item_tree::SourceLocations;
use rg_semantic_ir::{SemanticItemSource, SemanticItemView};
use rg_source::ReadError;

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
    body_ir: &'txn BodyReader<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyReader<'db>,
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
    pub(crate) fn scan(&self) -> Result<Vec<BodySourceCandidate>, ReadError> {
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
    fn body_at(&self) -> Result<Option<BodyRef>, ReadError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in
            self.body_ir
                .bodies_at(self.crate_ref, self.file_id, self.offset, None)?
        {
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
    ) -> Result<Vec<BodySourceCandidate>, ReadError> {
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
            let source = body.source_at(expr.source);
            if source.file_id != self.file_id {
                continue;
            }
            if !source.is_written() {
                self.consider_record_expr_fields(body_ref, body, expr, &mut best);
                let span = Self::expression_reference_span(body, expr).unwrap_or(source.span);
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
            if source.span.touches(self.offset) {
                if record_shorthand_values.contains(&ExprId(expr_idx)) {
                    continue;
                }
                // Record expression keys can resolve to fields, while the record expression itself
                // is still an ordinary expression candidate.
                self.consider_record_expr_fields(body_ref, body, expr, &mut best);
                let span = Self::expression_reference_span(body, expr)
                    .filter(|span| span.touches(self.offset))
                    .unwrap_or(source.span);
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
            let binding_span = binding
                .name_location
                .map(|id| body.span(id))
                .unwrap_or(body.source_at(binding.source).span);
            if body.source_at(binding.source).file_id != self.file_id
                || !body.contains_source_span(body.source_at(binding.source), binding_span)
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

        // Body-local items live in the same source range as the body but are stored in a local
        // item store. Their names can be the best answer when the cursor is on a nested item
        // declaration or one of its fields/variants.
        for item in self.body_ir.all_items(DefMapRef::Body(body_ref))? {
            if let ItemSourceKind::Body(source) = item.source().kind
                && source.body == body_ref
                && !body.source_item_is_written(source.item)
            {
                continue;
            }
            if item.source().file_id != self.file_id {
                continue;
            }
            let locations = self
                .body_ir
                .declaration_source(body_ref.crate_ref, item.source())?;
            let occurrence = match item.source().kind {
                ItemSourceKind::Body(source) if source.body == body_ref => body
                    .source_item(source.item)
                    .and_then(|node| node.name_location),
                _ => item.name_location(),
            };
            let Some(declaration_span) = locations.editable_span(occurrence) else {
                continue;
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
                self.consider_fields(item, ty, &locations, &mut best);
                self.consider_variants(item, ty, &locations, &mut best);
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
    ) -> Result<(), ReadError> {
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
            if body.source_at(call.source).file_id != self.file_id
                || !(body.source_at(call.source).is_written()
                    || body
                        .source_map()
                        .is_macro_argument(body.span(call.name_location)))
            {
                continue;
            }
            if !body.span(call.name_location).touches(self.offset) {
                continue;
            }

            best.consider(
                body.span(call.name_location),
                BodySourceCandidate::MacroCall {
                    body: body_ref,
                    definition: call.definition,
                    file_id: body.source_at(call.source).file_id,
                    span: body.span(call.name_location),
                },
            );
        }
    }

    fn body_owner_declaration(
        &self,
        body: BodyView<'_>,
    ) -> Result<Option<(SemanticItemRef, Span)>, ReadError> {
        let item = match body.owner() {
            BodyOwner::Function(function) => SemanticItemRef::from(function),
            BodyOwner::Const(konst) => SemanticItemRef::from(konst),
            BodyOwner::Static(static_ref) => SemanticItemRef::from(static_ref),
        };
        if !matches!(item.origin(), DefMapRef::Body(_)) {
            return Ok(None);
        }
        let Some(view) = self.body_ir.item(item)? else {
            return Ok(None);
        };
        let locations = self
            .body_ir
            .declaration_source(item.origin().origin_crate(), view.source())?;
        Ok(locations
            .editable_span(view.name_location())
            .map(|span| (item, span)))
    }

    /// Recognize macro ranges even when they produce no expression, such as type expansions.
    fn macro_source_touches_offset(&self, body: BodyView<'_>) -> bool {
        body.macro_calls().iter().any(|call| {
            let source = body.source_at(call.source);
            source.file_id == self.file_id && source.span.touches(self.offset)
        }) || body.exprs().iter().any(|expr| {
            let source = body.source_at(expr.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
        }) || body.pats().iter().any(|pat| {
            let source = body.source_at(pat.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
        }) || body.bindings().iter().any(|binding| {
            let source = body.source_at(binding.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
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
                || !body.span(field.key_location).touches(self.offset)
                || !body.contains_source_span(
                    body.source_at(expr.source),
                    body.span(field.key_location),
                )
            {
                continue;
            }
            best.consider_mapped(
                body.span(field.key_location),
                BodySourceCandidate::RecordFieldKey {
                    body: body_ref,
                    scope: expr.scope,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: body.source_at(expr.source).file_id,
                    span: body.span(field.key_location),
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
                    || !body.span(field.key_location).touches(self.offset)
                    || !body.contains_source_span(
                        body.source_at(site.data.source),
                        body.span(field.key_location),
                    )
                {
                    continue;
                }
                best.consider_mapped(
                    body.span(field.key_location),
                    BodySourceCandidate::RecordFieldKey {
                        body: body_ref,
                        scope: site.scope,
                        owner: owner.clone(),
                        key: field.key.clone(),
                        file_id: body.source_at(site.data.source).file_id,
                        span: body.span(field.key_location),
                        surface: RecordFieldKeySurface::Explicit,
                    },
                );
            }
        });
    }

    fn expression_reference_span(body: BodyView<'_>, expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::Path { path } => path.last_segment_location().map(|id| body.span(id)),
            ExprKind::MethodCall {
                method_name_location,
                ..
            } => method_name_location.map(|id| body.span(id)),
            ExprKind::Field { field_location, .. } => field_location.map(|id| body.span(id)),
            ExprKind::Record {
                path: Some(path), ..
            } => path.last_segment_location().map(|id| body.span(id)),
            _ => None,
        }
    }

    fn consider_fields(
        &self,
        item: SemanticItemView<'_>,
        ty: rg_ir_model::TypeDefRef,
        locations: &dyn SourceLocations,
        best: &mut BestCursorCandidate,
    ) {
        match ty.id {
            TypeDefId::Struct(_) => {
                let Some(data) = item.struct_data() else {
                    return;
                };
                if data.source.file_id != self.file_id {
                    return;
                }
                for (index, field) in data.fields.fields().iter().enumerate() {
                    let Some(span) = locations.editable_span(Some(field.location)) else {
                        continue;
                    };
                    if span.touches(self.offset) {
                        best.consider(
                            span,
                            BodySourceCandidate::LocalField {
                                field: FieldRef { owner: ty, index },
                                span,
                            },
                        );
                    }
                }
            }
            TypeDefId::Union(_) => {
                let Some(data) = item.union_data() else {
                    return;
                };
                if data.source.file_id != self.file_id {
                    return;
                }
                for (index, field) in data.fields.iter().enumerate() {
                    let Some(span) = locations.editable_span(Some(field.location)) else {
                        continue;
                    };
                    if span.touches(self.offset) {
                        best.consider(
                            span,
                            BodySourceCandidate::LocalField {
                                field: FieldRef { owner: ty, index },
                                span,
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
        item: SemanticItemView<'_>,
        ty: rg_ir_model::TypeDefRef,
        locations: &dyn SourceLocations,
        best: &mut BestCursorCandidate,
    ) {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return;
        };
        let Some(data) = item.enum_data() else {
            return;
        };
        if data.source.file_id != self.file_id {
            return;
        }
        for (index, variant) in data.variants.iter().enumerate() {
            let Some(span) = locations.editable_span(Some(variant.name_location)) else {
                continue;
            };
            if span.touches(self.offset) {
                best.consider(
                    span,
                    BodySourceCandidate::LocalEnumVariant {
                        variant: EnumVariantRef {
                            origin: ty.origin,
                            enum_id,
                            index,
                        },
                        span,
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
