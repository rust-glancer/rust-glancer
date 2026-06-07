//! Point-query scanning for editor requests at one source offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.

use rg_ir_model::{
    BindingId, BodyId, BodyRef, DefMapRef, EnumVariantRef, ExprId, FieldRef, SemanticItemRef,
    TargetRef, TypeDefId, hir::source::ItemSourceKind,
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{BodyIrReadTxn, BodyOwner, ExprData, ExprKind, PatKind, ResolvedBodyData};

use super::{
    super::{BindingSurface, BodyCursorCandidate, RecordFieldKeySurface},
    paths::{TypePathCursorScanner, ValuePathCursorScanner},
    record_pat_shorthand::RecordPatShorthandBinding,
    sites::BodyScanSites,
};

/// Scans one Body IR transaction for all cursor candidates at a source offset.
pub(crate) struct BodyCursorScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
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

    /// Returns body-local candidates that can answer an editor query at this exact offset.
    pub(crate) fn scan(&self) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        let Some(body_ref) = self.body_at()? else {
            return Ok(Vec::new());
        };
        let Some(body) = self.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        candidates.push(self.candidate_at_body(body_ref, body)?);
        TypePathCursorScanner {
            body_ref,
            body,
            file_id: Some(self.file_id),
            offset: Some(self.offset),
            candidates: &mut candidates,
        }
        .scan();
        ValuePathCursorScanner {
            body_ref,
            body,
            file_id: Some(self.file_id),
            offset: Some(self.offset),
            include_single_segment: false,
            candidates: &mut candidates,
        }
        .scan();

        Ok(candidates)
    }

    /// Finds the innermost enclosing body at the cursor offset.
    fn body_at(&self) -> Result<Option<BodyRef>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(BodyRef, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source().file_id != self.file_id || !body.source().span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            let body_len = body.source().span.len();
            if best.is_none_or(|(_, best_len)| body_len < best_len) {
                best = Some((body_ref, body_len));
            }
        }

        Ok(best.map(|(body_ref, _)| body_ref))
    }

    /// Picks the most precise source node in one body, falling back to the body itself.
    fn candidate_at_body(
        &self,
        body_ref: BodyRef,
        body: &ResolvedBodyData,
    ) -> Result<BodyCursorCandidate, PackageStoreError> {
        let mut best = BestCursorCandidate::new(BodyCursorCandidate::Body {
            body: body_ref,
            span: body.source().span,
        });
        let record_shorthand_values = Self::record_expr_shorthand_values(body);
        let record_shorthand_bindings = self.record_shorthand_bindings(body);

        // `body_at` chooses the innermost enclosing body. When the cursor is on a nested
        // fn/const/static declaration name, that body owns the initializer/block, while the
        // declaration item still lives in the parent body-local item store.
        self.consider_body_owner_declaration(body, &mut best)?;

        // First look at expressions under the cursor: calls, paths, field accesses, literals, and
        // so on. Record shorthand values are skipped here because the key token has its own
        // source-level candidate.
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            if expr.source.file_id == self.file_id && expr.source.span.touches(self.offset) {
                if record_shorthand_values.contains(&ExprId(expr_idx)) {
                    continue;
                }
                // Record expression keys can resolve to fields, while the record expression itself
                // is still an ordinary expression candidate.
                self.consider_record_expr_fields(body_ref, expr, &mut best);
                let span = Self::member_reference_span(expr)
                    .filter(|span| span.touches(self.offset))
                    .unwrap_or(expr.source.span);
                best.consider(
                    span,
                    BodyCursorCandidate::Expr {
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
            if binding.source.file_id == self.file_id && binding_span.touches(self.offset) {
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
                best.consider(
                    binding_span,
                    BodyCursorCandidate::Binding {
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
                            BodyCursorCandidate::LocalItem {
                                item: item.item(),
                                span: declaration_span,
                            },
                        ),
                        SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => best.consider(
                            declaration_span,
                            BodyCursorCandidate::LocalValueItem {
                                item: item.item(),
                                span: declaration_span,
                            },
                        ),
                        SemanticItemRef::Function(function) => best.consider(
                            declaration_span,
                            BodyCursorCandidate::LocalFunction {
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

        Ok(best.finish())
    }

    /// Projects a nested body back to its body-local item declaration when the cursor is on the
    /// owner name, so editor queries target the function/const/static item instead of the body.
    fn consider_body_owner_declaration(
        &self,
        body: &ResolvedBodyData,
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
                BodyCursorCandidate::LocalFunction {
                    function,
                    span: declaration_span,
                },
            ),
            SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => best.consider(
                declaration_span,
                BodyCursorCandidate::LocalValueItem {
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

    fn body_owner_declaration(
        &self,
        body: &ResolvedBodyData,
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

    fn record_shorthand_bindings(&self, body: &ResolvedBodyData) -> Vec<RecordPatShorthandBinding> {
        let mut bindings = Vec::new();
        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), |site| {
            let PatKind::Record { fields, .. } = &site.data.kind else {
                return;
            };

            for field in fields {
                let Some(shorthand) = RecordPatShorthandBinding::from_field(body, field) else {
                    continue;
                };
                if bindings
                    .iter()
                    .any(|seen: &RecordPatShorthandBinding| seen.binding == shorthand.binding)
                {
                    continue;
                }
                bindings.push(shorthand);
            }
        });
        bindings
    }

    fn record_expr_shorthand_values(body: &ResolvedBodyData) -> Vec<ExprId> {
        let mut values = Vec::new();
        for expr in body.exprs().iter() {
            let ExprKind::Record { fields, .. } = &expr.kind else {
                continue;
            };
            for field in fields {
                if field.syntax.is_explicit() {
                    continue;
                }
                if let Some(value) = field.value {
                    values.push(value);
                }
            }
        }
        values
    }

    fn consider_record_expr_fields(
        &self,
        body_ref: BodyRef,
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
            if !field.syntax.is_explicit() || !field.key_span.touches(self.offset) {
                continue;
            }
            best.consider(
                field.key_span,
                BodyCursorCandidate::RecordFieldKey {
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
        body: &ResolvedBodyData,
        best: &mut BestCursorCandidate,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), |site| {
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
                if !field.syntax.is_explicit() || !field.key_span.touches(self.offset) {
                    continue;
                }
                best.consider(
                    field.key_span,
                    BodyCursorCandidate::RecordFieldKey {
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

    fn member_reference_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::Path { path } if path.segment_count() == 1 => path.segment_span(0),
            ExprKind::MethodCall {
                method_name_span, ..
            } => *method_name_span,
            ExprKind::Field { field_span, .. } => *field_span,
            _ => None,
        }
    }

    fn consider_fields(
        &self,
        item_store: &rg_ir_storage::ItemStore,
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
                            BodyCursorCandidate::LocalField {
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
                            BodyCursorCandidate::LocalField {
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
        item_store: &rg_ir_storage::ItemStore,
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
                    BodyCursorCandidate::LocalEnumVariant {
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
struct BestCursorCandidate {
    candidate: Option<(u32, BodyCursorCandidate)>,
    fallback: BodyCursorCandidate,
}

impl BestCursorCandidate {
    fn new(fallback: BodyCursorCandidate) -> Self {
        Self {
            candidate: None,
            fallback,
        }
    }

    fn consider(&mut self, span: Span, candidate: BodyCursorCandidate) {
        let len = span.len();
        if self
            .candidate
            .as_ref()
            .is_none_or(|(best_len, _)| len < *best_len)
        {
            self.candidate = Some((len, candidate));
        }
    }

    fn finish(self) -> BodyCursorCandidate {
        self.candidate
            .map(|(_, candidate)| candidate)
            .unwrap_or(self.fallback)
    }
}
