//! Whole-target source scanning for project-wide body-local queries.
//!
//! Source scans collect every body-local declaration and reference-like span
//! that can participate in navigation, references, and symbol queries.

use rg_ir_model::{
    BindingId, BodyId, BodyRef, EnumVariantRef, ExprId, FieldRef, SemanticItemRef, TargetRef,
    TypeDefId, hir::source::ItemSourceKind,
};
use rg_ir_storage::BodyLocalItems;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{BodyIrReadTxn, ExprKind, PatKind, ResolvedBodyData};

use super::{
    super::{BindingSurface, BodyCursorCandidate, RecordFieldKeySurface},
    paths::{TypePathCursorScanner, ValuePathCursorScanner},
    record_pat_shorthand::RecordPatShorthandBinding,
    sites::BodyScanSites,
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
            if !self.file_matches(body.source().file_id) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };

            let body_local_items = target_bodies.body_local_items(body_ref.body);

            self.push_declaration_candidates(body_ref, body, body_local_items, &mut candidates);
            self.push_member_reference_candidates(body_ref, body, &mut candidates);
            self.push_record_field_key_candidates(body_ref, body, &mut candidates);

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
        body: &ResolvedBodyData,
        body_local_items: Option<&BodyLocalItems>,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        let record_shorthand_bindings = self.record_shorthand_bindings(body);
        for (binding_idx, binding) in body.bindings().iter().enumerate() {
            if !binding.source.is_written_in_selected_file(self.file_id) {
                continue;
            }
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
            let span = binding.name_span.unwrap_or(binding.source.span);
            candidates.push(BodyCursorCandidate::Binding {
                body: body_ref,
                binding: binding_id,
                span,
                surface,
            });
        }

        let Some(item_store) = body_local_items.map(BodyLocalItems::item_store) else {
            return;
        };
        for item in item_store.semantic_items() {
            if let ItemSourceKind::Body(source) = item.source().kind
                && source.body == body_ref
                && !body.source_item_is_written(source.item)
            {
                continue;
            }
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
                    .unwrap_or_else(|| item.span().unwrap_or(body.source().span)),
                _ => item.span().unwrap_or(body.source().span),
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

    fn record_shorthand_bindings(&self, body: &ResolvedBodyData) -> Vec<RecordPatShorthandBinding> {
        let mut bindings = Vec::new();
        let sites = BodyScanSites::new(body);
        sites.walk_pats(self.file_id, None, |site| {
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
        body: &ResolvedBodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        let record_shorthand_values = Self::record_expr_shorthand_values(body);
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            if !expr.source.is_written_in_selected_file(self.file_id) {
                continue;
            }
            if record_shorthand_values.contains(&ExprId(expr_idx)) {
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

    /// Adds record field keys that resolve through their record owner type.
    fn push_record_field_key_candidates(
        &self,
        body_ref: BodyRef,
        body: &ResolvedBodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        for expr in body.exprs().iter() {
            if !expr.source.is_written_in_selected_file(self.file_id) {
                continue;
            }
            let ExprKind::Record {
                path: Some(owner),
                fields,
                ..
            } = &expr.kind
            else {
                continue;
            };
            let Some(owner) = owner.as_def_map_path() else {
                continue;
            };

            for field in fields {
                candidates.push(BodyCursorCandidate::RecordFieldKey {
                    body: body_ref,
                    scope: expr.scope,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: expr.source.file_id,
                    span: field.key_span,
                    surface: if field.syntax.is_explicit() {
                        RecordFieldKeySurface::Explicit
                    } else {
                        RecordFieldKeySurface::RecordExprShorthand {
                            field_span: field.source_span,
                        }
                    },
                });
            }
        }

        let sites = BodyScanSites::new(body);
        sites.walk_pats(self.file_id, None, |site| {
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
                candidates.push(BodyCursorCandidate::RecordFieldKey {
                    body: body_ref,
                    scope: site.scope,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: site.data.source.file_id,
                    span: field.key_span,
                    surface: if field.syntax.is_explicit() {
                        RecordFieldKeySurface::Explicit
                    } else {
                        RecordFieldKeySurface::RecordPatShorthand {
                            field_span: field.source_span,
                            pat_span: body
                                .pat(field.pat)
                                .map(|pat| pat.source.span)
                                .unwrap_or(field.source_span),
                        }
                    },
                });
            }
        });
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }
}
