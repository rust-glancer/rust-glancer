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

use anyhow::Context as _;
use rg_body_ir::{BodyView, ExprKind, PatKind};
use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    BindingId, BodyRef, CrateRef, DefMapRef, EnumVariantRef, ExprId, FieldRef, FileId,
    SemanticItemRef, TypeDefId, TypeDefRef,
};
use rg_item_tree::SourceLocations;
use rg_semantic_ir::{ItemStoreQuery, SemanticItemView};

use super::{
    BindingSurface, BodySourceCandidate, RecordFieldKeySurface,
    paths::{BodyPathSourceScanner, TypePathSourceScanner},
    record_pat_shorthand::RecordPatShorthandBinding,
    sites::BodyScanSites,
};
use crate::{IndexedViewDb, source::ReferenceSourceSelection};

/// Scans one crate for every written body-local source candidate used by whole-project queries.
///
/// Format captures retain their exact written name ranges even though their path expressions are
/// lowered during expansion. Other generated expressions are represented by the written macro call.
pub(crate) struct BodySourceScanner<'txn, 'db> {
    db: &'txn IndexedViewDb<'db>,
    crate_ref: CrateRef,
    file_id: Option<FileId>,
    selection: Option<&'txn ReferenceSourceSelection>,
}

impl<'txn, 'db> BodySourceScanner<'txn, 'db> {
    pub(crate) fn new(
        db: &'txn IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
        selection: Option<&'txn ReferenceSourceSelection>,
    ) -> Self {
        Self {
            db,
            crate_ref,
            file_id,
            selection,
        }
    }

    /// Returns all body-local candidates in this crate, optionally limited to one file.
    pub(crate) fn scan(&self) -> anyhow::Result<Vec<BodySourceCandidate>> {
        let mut candidates = Vec::new();
        rg_std::check_cancel!(self.db, "body scan inventory");
        let files = match self.file_id {
            Some(file) => vec![file],
            None => self
                .db
                .body_ir
                .body_files(self.crate_ref)
                .context("list body scan files")?,
        };
        for file in files {
            rg_std::check_cancel!(self.db, "body scan file");
            let owners = self
                .selection
                .map(|selection| selection.owners(self.db, self.crate_ref, file))
                .transpose()?;
            for (body_ref, body) in self
                .db
                .body_ir
                .bodies_matching(self.crate_ref, Some(file), &|owner| {
                    owners
                        .as_ref()
                        .is_none_or(|owners| owners.contains(&owner.owner))
                })
                .context("load body scan file")?
            {
                rg_std::check_cancel!(self.db, "body scan");

                self.push_declaration_candidates(body_ref, body, &mut candidates)
                    .context("collect body occurrences")?;
                self.push_macro_call_candidates(body_ref, body, &mut candidates)
                    .context("collect body occurrences")?;
                self.push_expression_reference_candidates(body_ref, body, &mut candidates)
                    .context("collect body occurrences")?;
                self.push_record_field_key_candidates(body_ref, body, &mut candidates)
                    .context("collect body occurrences")?;

                TypePathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates)
                    .scan();
                BodyPathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates)
                    .scan();
            }
        }
        Ok(candidates)
    }

    /// Adds written macro invocations as references to the macro definition selected by expansion.
    fn push_macro_call_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        for call in body.macro_calls() {
            let source = body.source_at(call.source);
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            if !source.is_written_in_selected_file(self.file_id) {
                continue;
            }

            candidates.push(BodySourceCandidate::MacroCall {
                body: body_ref,
                definition: call.definition,
                file_id: source.file_id,
                span: body.span(call.name_location),
            });
        }
        Ok(())
    }

    /// Adds declarations using the spans users expect to navigate from: names and field names.
    fn push_declaration_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        let record_shorthand_bindings =
            RecordPatShorthandBinding::collect(body, self.file_id, None);
        for (binding_idx, binding) in body.bindings().iter().enumerate() {
            let source = body.source_at(binding.source);
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            if !source.is_written_in_selected_file(self.file_id) {
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
            let span = binding
                .name_location
                .map(|id| body.span(id))
                .unwrap_or(source.span);
            candidates.push(BodySourceCandidate::Binding {
                body: body_ref,
                binding: binding_id,
                span,
                surface,
            });
        }

        for item in
            ItemStoreQuery::new(self.db).semantic_items_for_origin(DefMapRef::Body(body_ref))?
        {
            rg_std::check_cancel!(self.db, "body occurrence candidates");
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

            let locations = self
                .db
                .declaration_source(item.item().origin(), item.source())?;
            let occurrence = match item.source().kind {
                ItemSourceKind::Body(source) if source.body == body_ref => body
                    .source_item(source.item)
                    .and_then(|node| node.name_location),
                _ => item.name_location(),
            };
            let Some(declaration_span) = locations.editable_span(occurrence) else {
                continue;
            };
            match item.item() {
                SemanticItemRef::TypeDef(ty) => {
                    candidates.push(BodySourceCandidate::LocalItem {
                        item: item.item(),
                        span: declaration_span,
                    });
                    self.push_field_candidates(item, ty, &locations, candidates)
                        .context("collect body occurrences")?;
                    self.push_variant_candidates(item, ty, &locations, candidates)
                        .context("collect body occurrences")?;
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
        Ok(())
    }

    fn push_field_candidates(
        &self,
        item: SemanticItemView<'_>,
        ty: TypeDefRef,
        locations: &dyn SourceLocations,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        match ty.id {
            TypeDefId::Struct(_) => {
                let Some(data) = item.struct_data() else {
                    return Ok(());
                };
                if !self.file_matches(data.source.file_id) {
                    return Ok(());
                }
                for (index, field) in data.fields.fields().iter().enumerate() {
                    rg_std::check_cancel!(self.db, "body occurrence candidates");
                    let Some(span) = locations.editable_span(Some(field.location)) else {
                        continue;
                    };
                    candidates.push(BodySourceCandidate::LocalField {
                        field: FieldRef { owner: ty, index },
                        span,
                    });
                }
            }
            TypeDefId::Union(_) => {
                let Some(data) = item.union_data() else {
                    return Ok(());
                };
                if !self.file_matches(data.source.file_id) {
                    return Ok(());
                }
                for (index, field) in data.fields.iter().enumerate() {
                    rg_std::check_cancel!(self.db, "body occurrence candidates");
                    let Some(span) = locations.editable_span(Some(field.location)) else {
                        continue;
                    };
                    candidates.push(BodySourceCandidate::LocalField {
                        field: FieldRef { owner: ty, index },
                        span,
                    });
                }
            }
            TypeDefId::Enum(_) => {}
        }
        Ok(())
    }

    fn push_variant_candidates(
        &self,
        item: SemanticItemView<'_>,
        ty: TypeDefRef,
        locations: &dyn SourceLocations,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(());
        };
        let Some(data) = item.enum_data() else {
            return Ok(());
        };
        for (index, variant) in data.variants.iter().enumerate() {
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(span) = locations.editable_span(Some(variant.name_location)) else {
                continue;
            };
            candidates.push(BodySourceCandidate::LocalEnumVariant {
                variant: EnumVariantRef {
                    origin: ty.origin,
                    enum_id,
                    index,
                },
                span,
            });
        }
        Ok(())
    }

    /// Adds the editable name span for expressions that resolve as one semantic reference.
    ///
    /// This selects `value` in a path, `push` in `items.push(value)`, `name` in `user.name`, and
    /// `User` in `model::User { name }` instead of indexing each expression's full source range.
    fn push_expression_reference_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        let record_shorthand_values = BodyScanSites::new(body).record_expr_shorthand_value_ids();
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            let source = body.source_at(expr.source);
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            let expr_id = ExprId(expr_idx);
            if self.file_id.is_some_and(|file| source.file_id != file)
                || !source.is_written()
                    && !body.source_map().is_format_capture(expr_id, source.span)
            {
                continue;
            }
            if record_shorthand_values.contains(&expr_id) {
                continue;
            }

            let span = match &expr.kind {
                ExprKind::Path { path }
                    if path.segment_count() == 1 && path.as_def_map_path().is_some() =>
                {
                    path.segment_location(0)
                        .map(|id| body.span(id))
                        .unwrap_or(source.span)
                }
                ExprKind::MethodCall {
                    method_name_location: Some(location),
                    ..
                }
                | ExprKind::Field {
                    field_location: Some(location),
                    ..
                } => body.span(*location),
                ExprKind::MethodCall { .. } | ExprKind::Field { .. } => source.span,
                ExprKind::Record {
                    path: Some(path), ..
                } => path
                    .last_segment_location()
                    .map(|id| body.span(id))
                    .unwrap_or(source.span),
                _ => continue,
            };

            candidates.push(BodySourceCandidate::Expr {
                body: body_ref,
                expr: expr_id,
                span,
            });
        }
        Ok(())
    }

    /// Adds record field keys that resolve through their record owner type.
    fn push_record_field_key_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        for expr in body.exprs().iter() {
            let source = body.source_at(expr.source);
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            if !source.is_written_in_selected_file(self.file_id) {
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
                rg_std::check_cancel!(self.db, "body occurrence candidates");
                candidates.push(BodySourceCandidate::RecordFieldKey {
                    body: body_ref,
                    scope: expr.scope,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: source.file_id,
                    span: body.span(field.key_location),
                    surface: if field.syntax.is_explicit() {
                        RecordFieldKeySurface::Explicit
                    } else {
                        RecordFieldKeySurface::RecordExprShorthand {
                            field_span: body.span(field.location),
                        }
                    },
                });
            }
        }

        let sites = BodyScanSites::new(body);
        sites.walk_pats(self.file_id, None, false, |site| {
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
                    file_id: body.source_at(site.data.source).file_id,
                    span: body.span(field.key_location),
                    surface: if field.syntax.is_explicit() {
                        RecordFieldKeySurface::Explicit
                    } else {
                        RecordFieldKeySurface::RecordPatShorthand {
                            field_span: body.span(field.location),
                            pat_span: body
                                .pat(field.pat)
                                .map(|pat| body.source_at(pat.source).span)
                                .unwrap_or(body.span(field.location)),
                        }
                    },
                });
            }
        });
        Ok(())
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }
}
