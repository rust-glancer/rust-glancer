//! Whole-crate source scanning for project-wide body-local queries.
//!
//! Source scans collect every body-local declaration and reference-like span
//! that can participate in navigation, references, and symbol queries.
//! Unlike a point scan, nothing is discarded merely because a narrower node overlaps it:
//!
//! ```text
//! let value = input;
//! consume(value);
//!     ^     ^ retain both references, as well as the `value` declaration
//! ```

use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    BindingId, BodyRef, CrateRef, EnumVariantRef, ExprId, FieldRef, SemanticItemRef, TypeDefId,
};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use rg_body_ir::{BodyIrReadTxn, BodyLocalItems, BodyView, ExprKind, PatKind};

use super::{
    BindingSurface, BodySourceCandidate, RecordFieldKeySurface,
    paths::{BodyPathSourceScanner, TypePathSourceScanner},
    record_pat_shorthand::RecordPatShorthandBinding,
    sites::BodyScanSites,
};

/// Scans one crate for every written body-local source candidate used by whole-project queries.
///
/// Generated expansion internals are deliberately skipped. Their written macro invocation is
/// emitted instead, so project-wide references point at source the user can edit.
pub(crate) struct BodySourceScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: Option<FileId>,
}

impl<'txn, 'db> BodySourceScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            body_ir,
            crate_ref,
            file_id,
        }
    }

    /// Returns all body-local candidates in this crate, optionally limited to one file.
    pub(crate) fn scan(&self) -> Result<Vec<BodySourceCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, self.file_id)? {
            let body_local_items = self.body_ir.body_local_items(body_ref)?;

            self.push_declaration_candidates(body_ref, body, body_local_items, &mut candidates);
            self.push_macro_call_candidates(body, &mut candidates);
            self.push_member_reference_candidates(body_ref, body, &mut candidates);
            self.push_record_field_key_candidates(body_ref, body, &mut candidates);

            TypePathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates).scan();
            BodyPathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates).scan();
        }

        Ok(candidates)
    }

    /// Adds written macro invocations as references to the macro definition selected by expansion.
    fn push_macro_call_candidates(
        &self,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) {
        for call in body.macro_calls() {
            if !call.source.is_written_in_selected_file(self.file_id) {
                continue;
            }

            candidates.push(BodySourceCandidate::MacroCall {
                definition: call.definition,
                file_id: call.source.file_id,
                span: call.name_span,
            });
        }
    }

    /// Adds declarations using the spans users expect to navigate from: names and field names.
    fn push_declaration_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        body_local_items: Option<&BodyLocalItems>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) {
        let record_shorthand_bindings =
            RecordPatShorthandBinding::collect(body, self.file_id, None);
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
            candidates.push(BodySourceCandidate::Binding {
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
                    candidates.push(BodySourceCandidate::LocalItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                    self.push_field_candidates(item_store, ty, candidates);
                    self.push_variant_candidates(item_store, ty, candidates);
                }
                SemanticItemRef::Trait(_) | SemanticItemRef::TypeAlias(_) => {
                    candidates.push(BodySourceCandidate::LocalItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                }
                SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => {
                    candidates.push(BodySourceCandidate::LocalValueItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                }
                SemanticItemRef::Function(function) => {
                    candidates.push(BodySourceCandidate::LocalFunction {
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
        item_store: &rg_semantic_ir::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        candidates: &mut Vec<BodySourceCandidate>,
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
                    candidates.push(BodySourceCandidate::LocalField {
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
                    candidates.push(BodySourceCandidate::LocalField {
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
        item_store: &rg_semantic_ir::ItemStore,
        ty: rg_ir_model::TypeDefRef,
        candidates: &mut Vec<BodySourceCandidate>,
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
            candidates.push(BodySourceCandidate::LocalEnumVariant {
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
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) {
        let record_shorthand_values = BodyScanSites::new(body).record_expr_shorthand_value_ids();
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

            candidates.push(BodySourceCandidate::Expr {
                body: body_ref,
                expr: ExprId(expr_idx),
                span,
            });
        }
    }

    /// Adds record field keys that resolve through their record owner type.
    fn push_record_field_key_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
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
                candidates.push(BodySourceCandidate::RecordFieldKey {
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
                candidates.push(BodySourceCandidate::RecordFieldKey {
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
