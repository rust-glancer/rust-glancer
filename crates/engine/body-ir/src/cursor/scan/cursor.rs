//! Point-query scanning for editor requests at one source offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.

use rg_ir_model::{
    BindingId, BodyId, BodyRef, EnumVariantRef, ExprId, FieldRef, SemanticItemRef, TargetRef,
    TypeDefId, hir::source::ItemSourceKind,
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{BodyData, BodyIrReadTxn};

use super::{
    super::BodyCursorCandidate,
    paths::{TypePathCursorScanner, ValuePathCursorScanner},
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
        candidates.push(self.candidate_at_body(body_ref, body));
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
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            let body_len = body.source.span.len();
            if best.is_none_or(|(_, best_len)| body_len < best_len) {
                best = Some((body_ref, body_len));
            }
        }

        Ok(best.map(|(body_ref, _)| body_ref))
    }

    /// Picks the most precise source node in one body, falling back to the body itself.
    fn candidate_at_body(&self, body_ref: BodyRef, body: &BodyData) -> BodyCursorCandidate {
        let mut best = BestCursorCandidate::new(BodyCursorCandidate::Body {
            body: body_ref,
            span: body.source.span,
        });

        for (expr_idx, expr) in body.exprs.iter().enumerate() {
            if expr.source.file_id == self.file_id && expr.source.span.touches(self.offset) {
                best.consider(
                    expr.source.span,
                    BodyCursorCandidate::Expr {
                        body: body_ref,
                        expr: ExprId(expr_idx),
                        span: expr.source.span,
                    },
                );
            }
        }

        for (binding_idx, binding) in body.bindings.iter().enumerate() {
            if binding.source.file_id == self.file_id && binding.source.span.touches(self.offset) {
                best.consider(
                    binding.source.span,
                    BodyCursorCandidate::Binding {
                        body: body_ref,
                        binding: BindingId(binding_idx),
                        span: binding.source.span,
                    },
                );
            }
        }

        if let Some(item_store) = body.body_item_store() {
            for item in item_store.semantic_items() {
                if item.source().file_id != self.file_id {
                    continue;
                }
                let declaration_span = match item.source().kind {
                    ItemSourceKind::Body(source) if source.body == body_ref => body
                        .source_item(source.item)
                        .and_then(|item| item.name_span)
                        .unwrap_or_else(|| item.span().unwrap_or(body.source.span)),
                    _ => item.span().unwrap_or(body.source.span),
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

        best.finish()
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
