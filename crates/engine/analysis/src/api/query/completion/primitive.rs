//! Primitive type completion assembly.
//!
//! Primitives are part of the Rust language rather than module-scope definitions, so completion
//! renders them from the Body IR primitive vocabulary instead of pretending they live in DefMap.

use rg_body_ir::{BodyPrimitiveTy, UnqualifiedCompletionSite};
use rg_def_map::Path;

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    def_completion_detail,
};

pub(super) struct PrimitiveTypeCompletionResolver;

impl PrimitiveTypeCompletionResolver {
    pub(super) fn body_completions(
        analysis: &Analysis<'_>,
        site: &UnqualifiedCompletionSite,
        edit: CompletionEdit,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let mut completions = Vec::new();

        for primitive in BodyPrimitiveTy::ALL
            .iter()
            .copied()
            .filter(|primitive| primitive.label().starts_with(&site.member_prefix))
        {
            // Use Body IR type resolution as the single source of truth for primitive shadowing.
            let path = Path::unqualified_name(primitive.label());
            let resolution = analysis.body_ir.resolve_type_path_in_scope(
                &analysis.def_map,
                &analysis.semantic_ir,
                site.body,
                site.scope,
                &path,
            )?;
            if resolution.is_primitive(&primitive) {
                completions.push(Self::completion(primitive, edit));
            }
        }

        Ok(completions)
    }

    fn completion(primitive: BodyPrimitiveTy, edit: CompletionEdit) -> CompletionItem {
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
