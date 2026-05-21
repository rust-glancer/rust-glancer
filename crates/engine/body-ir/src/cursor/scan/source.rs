//! Whole-target source scanning for project-wide body-local queries.
//!
//! Source scans collect every body-local declaration and reference-like span
//! that can participate in navigation, references, and symbol queries.

use rg_def_map::TargetRef;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{
    BindingId, BodyData, BodyEnumVariantRef, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId,
    BodyIrReadTxn, BodyItemId, BodyItemRef, BodyRef, BodyValueItemId, BodyValueItemRef, ExprId,
    ExprKind,
};

use super::{
    super::BodyCursorCandidate,
    paths::{TypePathCursorScanner, ValuePathCursorScanner},
};

/// Scans one target for every body-local source candidate used by whole-project queries.
pub(crate) struct BodySourceScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: Option<FileId>,
}

impl<'txn, 'db> BodySourceScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            body_ir,
            target,
            file_id,
        }
    }

    /// Returns all body-local candidates in this target, optionally limited to one file.
    pub(crate) fn scan(&self) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if !self.file_matches(body.source.file_id) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };

            self.push_declaration_candidates(body_ref, body, &mut candidates);
            self.push_member_reference_candidates(body_ref, body, &mut candidates);

            TypePathCursorScanner {
                body_ref,
                body,
                file_id: self.file_id,
                offset: None,
                candidates: &mut candidates,
            }
            .scan();
            ValuePathCursorScanner {
                body_ref,
                body,
                file_id: self.file_id,
                offset: None,
                include_single_segment: true,
                candidates: &mut candidates,
            }
            .scan();
        }

        Ok(candidates)
    }

    /// Adds declarations using the spans users expect to navigate from: names and field names.
    fn push_declaration_candidates(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        for (binding_idx, binding) in body.bindings().iter().enumerate() {
            if !self.file_matches(binding.source.file_id) {
                continue;
            }
            candidates.push(BodyCursorCandidate::Binding {
                body: body_ref,
                binding: BindingId(binding_idx),
                span: binding.source.span,
            });
        }

        for (item_idx, item) in body.local_items().iter().enumerate() {
            if !self.file_matches(item.name_source.file_id) {
                continue;
            }

            let item_ref = BodyItemRef {
                body: body_ref,
                item: BodyItemId(item_idx),
            };
            candidates.push(BodyCursorCandidate::LocalItem {
                item: item_ref,
                span: item.name_source.span,
            });

            for (field_idx, field) in item.fields().iter().enumerate() {
                candidates.push(BodyCursorCandidate::LocalField {
                    field: BodyFieldRef {
                        item: item_ref,
                        index: field_idx,
                    },
                    span: field.span,
                });
            }

            for (variant_idx, variant) in item.enum_variants().iter().enumerate() {
                candidates.push(BodyCursorCandidate::LocalEnumVariant {
                    variant: BodyEnumVariantRef {
                        item: item_ref,
                        index: variant_idx,
                    },
                    span: variant.name_span,
                });
            }
        }

        for (item_idx, item) in body.local_value_items().iter().enumerate() {
            if !self.file_matches(item.name_source.file_id) {
                continue;
            }

            candidates.push(BodyCursorCandidate::LocalValueItem {
                item: BodyValueItemRef {
                    body: body_ref,
                    item: BodyValueItemId(item_idx),
                },
                span: item.name_source.span,
            });
        }

        for (function_idx, function) in body.local_functions().iter().enumerate() {
            if !self.file_matches(function.name_source.file_id) {
                continue;
            }
            candidates.push(BodyCursorCandidate::LocalFunction {
                function: BodyFunctionRef {
                    body: body_ref,
                    function: BodyFunctionId(function_idx),
                },
                span: function.name_source.span,
            });
        }
    }

    /// Adds reference-like candidates whose useful span is narrower than the full expression.
    fn push_member_reference_candidates(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            if !self.file_matches(expr.source.file_id) {
                continue;
            }

            let span = match &expr.kind {
                ExprKind::Path { path }
                    if path.segment_count() == 1 && path.as_def_map_path().is_some() =>
                {
                    path.segment_span(0).unwrap_or(expr.source.span)
                }
                ExprKind::MethodCall {
                    method_name_span: Some(span),
                    ..
                }
                | ExprKind::Field {
                    field_span: Some(span),
                    ..
                } => *span,
                ExprKind::MethodCall { .. } | ExprKind::Field { .. } => expr.source.span,
                _ => continue,
            };

            candidates.push(BodyCursorCandidate::Expr {
                body: body_ref,
                expr: ExprId(expr_idx),
                span,
            });
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }
}
