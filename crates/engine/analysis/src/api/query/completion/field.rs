//! Shared field-completion rendering.

use rg_ir_view::{IndexedViewDb, member::MemberField, signature::SignatureRenderer};

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::completion_sort::CompletionSortPolicy;

pub(super) struct FieldCompletionRenderer<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> FieldCompletionRenderer<'a, 'db> {
    pub(super) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    /// Builds one completion item for a resolved field declaration.
    pub(super) fn completion(
        &self,
        field: MemberField<'_>,
        edit: CompletionEdit,
    ) -> Option<CompletionItem> {
        let target = CompletionTarget::Field(field.field_ref());
        let label = field.key()?.to_string();
        let renderer = SignatureRenderer::new(self.0);

        Some(CompletionItem {
            label: label.clone(),
            kind: CompletionKind::Field,
            target,
            applicability: CompletionApplicability::Known,
            detail: renderer.member_field_signature(&field),
            documentation: field.docs_text(),
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &label,
                CompletionKind::Field,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        })
    }
}
