//! Shared field-completion rendering.

use rg_ir_view::{
    display::{signature::SignatureRenderer, syntax::SyntaxRenderer},
    member::MemberField,
};

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::completion_sort::CompletionSortPolicy;

pub(super) struct FieldCompletionRenderer {
    syntax: SyntaxRenderer,
}

impl FieldCompletionRenderer {
    pub(super) fn new(syntax: SyntaxRenderer) -> Self {
        Self { syntax }
    }

    /// Builds one completion item for a resolved field declaration.
    pub(super) fn completion(
        &self,
        field: MemberField<'_>,
        edit: CompletionEdit,
    ) -> Option<CompletionItem> {
        let target = CompletionTarget::Field(field.field_ref());
        let label = self.syntax.field_key(field.key()?).to_string();

        Some(CompletionItem {
            label: label.clone(),
            kind: CompletionKind::Field,
            target,
            applicability: CompletionApplicability::Known,
            detail: SignatureRenderer::new(self.syntax.edition()).member_field_signature(field),
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
