//! Whole-target source scanning for project-wide body-local queries.
//!
//! Source scans collect every body-local declaration and reference-like span
//! that can participate in navigation, references, and symbol queries.

use rg_ir_model::{
    BindingId, BodyId, BodyRef, EnumVariantRef, ExprId, FieldRef, SemanticItemRef, TargetRef,
    TypeDefId, hir::source::ItemSourceKind,
};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{BodyData, BodyIrReadTxn, ExprKind};

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

        let Some(item_store) = body.body_item_store() else {
            return;
        };
        for item in item_store.semantic_items() {
            if self
                .file_id
                .is_some_and(|file_id| item.source().file_id != file_id)
            {
                continue;
            }

            let declaration_span = match item.source().kind {
                ItemSourceKind::Body(source) if source.body == body_ref => body
                    .source_item(source.item)
                    .and_then(|item| item.name_span)
                    .unwrap_or_else(|| item.span().unwrap_or(body.source.span)),
                _ => item.span().unwrap_or(body.source.span),
            };

            match item.item() {
                SemanticItemRef::TypeDef(ty) => {
                    candidates.push(BodyCursorCandidate::LocalItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                    self.push_field_candidates(item_store, ty, candidates);
                    self.push_variant_candidates(item_store, ty, candidates);
                }
                SemanticItemRef::Trait(_) | SemanticItemRef::TypeAlias(_) => {
                    candidates.push(BodyCursorCandidate::LocalItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                }
                SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => {
                    candidates.push(BodyCursorCandidate::LocalValueItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                }
                SemanticItemRef::Function(function) => {
                    candidates.push(BodyCursorCandidate::LocalFunction {
                        function,
                        span: declaration_span,
                    });
                }
                SemanticItemRef::Impl(_) => {}
            }
        }
    }

    fn push_field_candidates(
        &self,
        item_store: &rg_ir_storage::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = item_store.struct_data(id) else {
                    return;
                };
                if !self.file_matches(data.source.file_id) {
                    return;
                }
                for (index, field) in data.fields.fields().iter().enumerate() {
                    candidates.push(BodyCursorCandidate::LocalField {
                        field: FieldRef { owner: ty, index },
                        span: field.span,
                    });
                }
            }
            TypeDefId::Union(id) => {
                let Some(data) = item_store.union_data(id) else {
                    return;
                };
                if !self.file_matches(data.source.file_id) {
                    return;
                }
                for (index, field) in data.fields.iter().enumerate() {
                    candidates.push(BodyCursorCandidate::LocalField {
                        field: FieldRef { owner: ty, index },
                        span: field.span,
                    });
                }
            }
            TypeDefId::Enum(_) => {}
        }
    }

    fn push_variant_candidates(
        &self,
        item_store: &rg_ir_storage::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return;
        };
        let Some(data) = item_store.enum_data(enum_id) else {
            return;
        };
        for (index, variant) in data.variants.iter().enumerate() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            candidates.push(BodyCursorCandidate::LocalEnumVariant {
                variant: EnumVariantRef {
                    origin: ty.origin,
                    enum_id,
                    index,
                },
                span: variant.name_span,
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
