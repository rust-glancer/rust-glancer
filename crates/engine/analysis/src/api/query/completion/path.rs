//! Qualified path completion assembly for body and import positions.

use crate::{
    Analysis,
    api::{
        completion_site::{PathCompletionContext, PathCompletionSite},
        view::{
            completion::{CompletionScopeNamespace, CompletionView, ModuleCompletionCandidate},
            enum_variant::{EnumVariant, EnumVariantView},
        },
    },
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionQuery,
    completion_sort::CompletionSortPolicy,
    def_completion_detail,
    function::FunctionCallCompletion,
    module_scope::{ModuleCompletionRenderer, ModuleCompletionRequest},
};

pub(super) struct PathCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> PathCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects qualified path completions, such as `crate::$0` or `use crate::api::$0`.
    pub(super) fn completions(
        &self,
        site: PathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let context = site.context();
        let completion_view = CompletionView::new(self.analysis);
        let mut completions = self.module_path_completions(
            completion_view.module_candidates_for_path(&site)?,
            edit,
            PathCompletionFilter::from(context),
            match context {
                PathCompletionContext::Type | PathCompletionContext::Value => {
                    FunctionCallCompletion::FunctionCall
                }
                PathCompletionContext::Import => FunctionCallCompletion::Plain,
            },
        )?;

        let enum_variants = EnumVariantView::new(self.analysis);
        for variant in completion_view.enum_variant_candidates_for_path(&site)? {
            let Some(variant) = enum_variants.variant(variant)? else {
                continue;
            };
            self.push_enum_variant_completion(variant, edit, &mut completions);
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));

        Ok(completions)
    }

    /// Renders definitions visible from a resolved module qualifier.
    fn module_path_completions(
        &self,
        candidates: Vec<ModuleCompletionCandidate>,
        edit: CompletionEdit,
        filter: PathCompletionFilter,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let renderer = ModuleCompletionRenderer::new(self.analysis, self.query);
        let mut completions: Vec<CompletionItem> = Vec::new();

        for candidate in candidates {
            if !filter.accepts(candidate.namespace()) {
                continue;
            }
            let Some(completion) = renderer.completion(ModuleCompletionRequest {
                candidate: &candidate,
                edit,
                function_call_completion,
                sort_policy: CompletionSortPolicy::General,
                sort_priority: None,
            })?
            else {
                continue;
            };
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_enum_variant_completion(
        &self,
        variant: EnumVariant<'_>,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) {
        let target = CompletionTarget::EnumVariant(variant.variant_ref());
        let label = variant.name();
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == label)
        {
            return;
        }

        completions.push(CompletionItem {
            label: label.to_string(),
            kind: CompletionKind::EnumVariant,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(CompletionKind::EnumVariant, label)),
            documentation: variant.docs_text(),
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                label,
                CompletionKind::EnumVariant,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
    }
}

/// Namespace policy for the segment being completed in a qualified path.
///
/// Type positions like `let value: crate::$0` accept type-namespace candidates.
/// Value positions like `let value = crate::$0` accept all candidates so modules
/// and types can still be used as prefixes on the way to a value item such as
/// `crate::api::build_user()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCompletionFilter {
    Types,
    All,
}

impl PathCompletionFilter {
    fn accepts(self, namespace: CompletionScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, CompletionScopeNamespace::Types),
            Self::All => true,
        }
    }
}

impl From<PathCompletionContext> for PathCompletionFilter {
    fn from(context: PathCompletionContext) -> Self {
        match context {
            PathCompletionContext::Type => Self::Types,
            PathCompletionContext::Value | PathCompletionContext::Import => Self::All,
        }
    }
}
