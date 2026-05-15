//! Shared field-completion rendering.
//!
//! Dot completions and record-field completions both render the same field metadata: label,
//! signature detail, docs, sort text, and replacement edit.

use rg_body_ir::{FieldKey, ResolvedFieldRef};
use rg_semantic_ir::Documentation;

use crate::{
    Analysis,
    api::render::signature::SignatureRenderer,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind, CompletionTarget,
    },
};

use super::{CompletionMetadata, completion_sort::CompletionSortPolicy};

pub(super) struct FieldCompletionRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> FieldCompletionRenderer<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Builds one completion item for a resolved field declaration.
    pub(super) fn completion(
        &self,
        field: ResolvedFieldRef,
        edit: CompletionEdit,
    ) -> anyhow::Result<Option<FieldCompletion>> {
        let Some(metadata) = self.field_completion_metadata(field)? else {
            return Ok(None);
        };
        let target = CompletionTarget::Field(field);

        Ok(Some(FieldCompletion {
            key: metadata.key,
            item: CompletionItem {
                label: metadata.completion.label.clone(),
                kind: CompletionKind::Field,
                target,
                applicability: CompletionApplicability::Known,
                detail: metadata.completion.detail,
                documentation: metadata.completion.documentation,
                sort_text: CompletionSortPolicy::General.sort_text(
                    None,
                    &metadata.completion.label,
                    CompletionKind::Field,
                    CompletionApplicability::Known,
                    target,
                ),
                edit: Some(edit),
            },
        }))
    }

    fn field_completion_metadata(
        &self,
        field: ResolvedFieldRef,
    ) -> anyhow::Result<Option<FieldCompletionMetadata>> {
        let renderer = SignatureRenderer::new(self.0);
        match field {
            ResolvedFieldRef::Semantic(field) => {
                let Some(data) = self.0.semantic_ir.field_data(field)? else {
                    return Ok(None);
                };
                let Some(key) = data.field.key.clone() else {
                    return Ok(None);
                };
                Ok(Some(FieldCompletionMetadata {
                    completion: CompletionMetadata {
                        label: key.to_string(),
                        detail: renderer.field_signature(data),
                        documentation: data.field.docs.as_ref().map(Documentation::text),
                    },
                    key,
                }))
            }
            ResolvedFieldRef::BodyLocal(field) => {
                let Some(data) = self.0.body_ir.local_field_data(field)? else {
                    return Ok(None);
                };
                let Some(key) = data.field.key.clone() else {
                    return Ok(None);
                };
                Ok(Some(FieldCompletionMetadata {
                    completion: CompletionMetadata {
                        label: key.to_string(),
                        detail: renderer.local_field_signature(data),
                        documentation: data.field.docs.as_ref().map(Documentation::text),
                    },
                    key,
                }))
            }
        }
    }
}

/// Rendered field completion plus its source-level field key.
pub(super) struct FieldCompletion {
    pub(super) key: FieldKey,
    pub(super) item: CompletionItem,
}

struct FieldCompletionMetadata {
    key: FieldKey,
    completion: CompletionMetadata,
}
