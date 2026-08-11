//! Shared field-completion rendering.

use rg_ir_view::{
    display::{signature::SignatureRenderer, syntax::SyntaxRenderer},
    member::{MemberEnumVariantField, MemberField},
};

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::sort::CompletionSortPolicy;

/// Renders named fields from both ordinary records and enum-variant records.
///
/// Candidate lookup has already selected an owner. This type is responsible only for stable field
/// identity, source spelling, signature/docs projection, and the final edit range.
pub(crate) struct FieldCompletionRenderer {
    syntax: SyntaxRenderer,
}

impl FieldCompletionRenderer {
    pub(crate) fn new(syntax: SyntaxRenderer) -> Self {
        Self { syntax }
    }

    /// Builds one completion item for a resolved field declaration.
    pub(crate) fn completion(
        &self,
        field: MemberField<'_>,
        edit: CompletionEdit,
    ) -> Option<CompletionItem> {
        self.completion_item(
            field.key()?,
            CompletionTarget::Field(field.field_ref()),
            SignatureRenderer::new(self.syntax.edition()).member_field_signature(field),
            field.docs_text(),
            edit,
        )
    }

    /// Builds one completion item for a field declared below an enum variant.
    pub(crate) fn enum_variant_completion(
        &self,
        field: MemberEnumVariantField<'_>,
        edit: CompletionEdit,
    ) -> Option<CompletionItem> {
        self.completion_item(
            field.key()?,
            CompletionTarget::EnumVariantField(field.field_ref()),
            SignatureRenderer::new(self.syntax.edition()).enum_variant_field_signature(field),
            field.docs_text(),
            edit,
        )
    }

    fn completion_item(
        &self,
        key: &rg_ir_model::FieldKey,
        target: CompletionTarget,
        detail: Option<String>,
        documentation: Option<String>,
        edit: CompletionEdit,
    ) -> Option<CompletionItem> {
        let label = self.syntax.field_key(key).to_string();
        Some(CompletionItem {
            label: label.clone(),
            filter_text: None,
            kind: CompletionKind::Field,
            target,
            applicability: CompletionApplicability::Known,
            detail,
            documentation,
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &label,
                CompletionKind::Field,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
            additional_edits: Vec::new(),
        })
    }
}
