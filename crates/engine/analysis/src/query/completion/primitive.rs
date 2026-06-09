//! Primitive type completion assembly.
//!
//! Primitives are part of the Rust language rather than module-scope definitions, so completion
//! renders them from the shared type vocabulary instead of pretending they live in DefMap.

use rg_ir_model::items::PrimitiveTy;

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::{
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    def_completion_detail,
};

pub(super) struct PrimitiveTypeCompletionResolver;

impl PrimitiveTypeCompletionResolver {
    pub(super) fn completions(
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
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(kind, &label)),
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
        }
    }
}
