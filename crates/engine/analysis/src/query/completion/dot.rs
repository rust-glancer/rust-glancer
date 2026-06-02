//! Dot-completion assembly for member access sites.

use rg_ir_view::ty::member::MemberView;

use crate::{
    Analysis,
    completion_site::DotCompletionSite,
    model::{CompletionEdit, CompletionItem, CompletionTarget},
};

use super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    completion_sort::CompletionSortPolicy,
    field::FieldCompletionRenderer,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
};

pub(super) struct DotCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> DotCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects member completions for a dot site like `user.na$0`.
    pub(super) fn completions(
        &self,
        site: DotCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let completion_candidates = CompletionCandidateSource::new(self.analysis.view_db());
        let members = MemberView::new(self.analysis.view_db());
        let mut completions = Vec::new();

        let field_renderer = FieldCompletionRenderer::new();
        for field_ref in completion_candidates.field_candidates_for_dot(&site)? {
            let Some(field) = members.field(field_ref)? else {
                continue;
            };
            let Some(completion) = field_renderer.completion(field, edit) else {
                continue;
            };
            if completions
                .iter()
                .any(|existing: &CompletionItem| existing.target == completion.target)
            {
                continue;
            }

            completions.push(completion);
        }

        let function_renderer = FunctionCompletionRenderer::new(self.query);
        for method in completion_candidates.method_candidates_for_dot(&site)? {
            let Some(function) = members.function(method.function_ref())? else {
                continue;
            };
            let target = CompletionTarget::Function(function.function_ref());
            let completion = function_renderer.completion(FunctionCompletionRequest {
                function,
                label_override: None,
                kind: method.kind(),
                applicability: method.applicability(),
                edit,
                call_completion: FunctionCallCompletion::MethodCall,
                sort_policy: CompletionSortPolicy::General,
                sort_priority: None,
            });
            if !completion.has_self_receiver
                || completions
                    .iter()
                    .any(|completion| completion.target == target)
            {
                continue;
            }

            completions.push(completion.item);
        }

        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
