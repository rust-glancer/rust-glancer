//! Shared field-completion rendering.
//!
//! Dot completions and record-field completions both render the same field metadata: label,
//! signature detail, docs, sort text, and replacement edit.

use rg_body_ir::FieldKey;

use crate::{
    Analysis,
    api::{render::signature::SignatureRenderer, view::member::MemberField},
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::completion_sort::CompletionSortPolicy;

pub(super) struct FieldCompletionRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> FieldCompletionRenderer<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Builds one completion item for a resolved field declaration.
    pub(super) fn completion(
        &self,
        field: MemberField<'_>,
        edit: CompletionEdit,
    ) -> Option<FieldCompletion> {
        let target = CompletionTarget::Field(field.field_ref());
        let Some(metadata) = self.field_completion_metadata(field) else {
            return None;
        };

        Some(FieldCompletion {
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
                insert_text: CompletionInsertText::Plain,
                edit: Some(edit),
            },
        })
    }

    fn field_completion_metadata(&self, field: MemberField<'_>) -> Option<FieldCompletionMetadata> {
        let Some(key) = field.key().cloned() else {
            return None;
        };
        let renderer = SignatureRenderer::new(self.0);

        Some(FieldCompletionMetadata {
            completion: CompletionMetadata {
                label: key.to_string(),
                detail: renderer.member_field_signature(&field),
                documentation: field.docs_text(),
            },
            key,
        })
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

struct CompletionMetadata {
    label: String,
    detail: Option<String>,
    documentation: Option<String>,
}
