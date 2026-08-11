//! Primitive type completion assembly.
//!
//! Primitives are part of the Rust language rather than module-scope definitions, so completion
//! renders them from the shared type vocabulary instead of pretending they live in DefMap.

use rg_ir_model::PrimitiveTy;

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::{
    definition_detail,
    sort::{CompletionSortPolicy, CompletionSortPriority},
};

/// Renders primitive types after scope lookup proves their spelling is not shadowed.
pub(crate) struct PrimitiveTypeCompletionRenderer;

impl PrimitiveTypeCompletionRenderer {
    /// Render the primitive set already filtered for prefix and scope shadowing.
    pub(crate) fn completions(
        primitives: impl IntoIterator<Item = PrimitiveTy>,
        edit: CompletionEdit,
    ) -> Vec<CompletionItem> {
        primitives
            .into_iter()
            .map(|primitive| Self::completion(primitive, edit))
            .collect()
    }

    fn completion(primitive: PrimitiveTy, edit: CompletionEdit) -> CompletionItem {
        let label = primitive.label().to_string();
        let kind = CompletionKind::PrimitiveType;
        let target = CompletionTarget::PrimitiveType(primitive);

        CompletionItem {
            label: label.clone(),
            filter_text: None,
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(definition_detail(kind, &label)),
            documentation: None,
            sort_text: CompletionSortPolicy::TypePosition.sort_text(
                Some(CompletionSortPriority::Primitive),
                &label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
            additional_edits: Vec::new(),
        }
    }
}
