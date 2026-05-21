//! Point-query scanning for editor requests at one source offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.

use rg_def_map::TargetRef;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{
    BindingId, BodyData, BodyEnumVariantRef, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId,
    BodyIrReadTxn, BodyItemId, BodyItemRef, BodyRef, BodyValueItemId, BodyValueItemRef, ExprId,
};

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

        for (item_idx, item) in body.local_items.iter().enumerate() {
            if item.name_source.file_id == self.file_id
                && item.name_source.span.touches(self.offset)
            {
                let item_ref = BodyItemRef {
                    body: body_ref,
                    item: BodyItemId(item_idx),
                };
                best.consider(
                    item.name_source.span,
                    BodyCursorCandidate::LocalItem {
                        item: item_ref,
                        span: item.name_source.span,
                    },
                );
            }
        }

        for (item_idx, item) in body.local_value_items.iter().enumerate() {
            if item.name_source.file_id == self.file_id
                && item.name_source.span.touches(self.offset)
            {
                best.consider(
                    item.name_source.span,
                    BodyCursorCandidate::LocalValueItem {
                        item: BodyValueItemRef {
                            body: body_ref,
                            item: BodyValueItemId(item_idx),
                        },
                        span: item.name_source.span,
                    },
                );
            }
        }

        for (item_idx, item) in body.local_items.iter().enumerate() {
            if item.source.file_id != self.file_id {
                continue;
            }

            let item_ref = BodyItemRef {
                body: body_ref,
                item: BodyItemId(item_idx),
            };
            for (field_idx, field) in item.fields().iter().enumerate() {
                if field.span.touches(self.offset) {
                    best.consider(
                        field.span,
                        BodyCursorCandidate::LocalField {
                            field: BodyFieldRef {
                                item: item_ref,
                                index: field_idx,
                            },
                            span: field.span,
                        },
                    );
                }
            }

            for (variant_idx, variant) in item.enum_variants().iter().enumerate() {
                if variant.name_span.touches(self.offset) {
                    best.consider(
                        variant.name_span,
                        BodyCursorCandidate::LocalEnumVariant {
                            variant: BodyEnumVariantRef {
                                item: item_ref,
                                index: variant_idx,
                            },
                            span: variant.name_span,
                        },
                    );
                }
            }
        }

        for (function_idx, function) in body.local_functions.iter().enumerate() {
            if function.name_source.file_id == self.file_id
                && function.name_source.span.touches(self.offset)
            {
                best.consider(
                    function.name_source.span,
                    BodyCursorCandidate::LocalFunction {
                        function: BodyFunctionRef {
                            body: body_ref,
                            function: BodyFunctionId(function_idx),
                        },
                        span: function.name_source.span,
                    },
                );
            }
        }

        best.finish()
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
