//! Dot-completion assembly for member access sites.

use rg_body_ir::DotCompletionSite;

use crate::{
    Analysis,
    api::view::{completion::CompletionView, member::MemberView},
    model::{CompletionEdit, CompletionItem, CompletionTarget},
};

use super::{
    CompletionQuery,
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
            replace: site.member_prefix_span,
        };
        let completion_view = CompletionView::new(self.analysis);
        let members = MemberView::new(self.analysis);
        let mut completions = Vec::new();

        let field_renderer = FieldCompletionRenderer::new(self.analysis);
        for field_ref in completion_view.field_candidates_for_dot(&site)? {
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

        let function_renderer = FunctionCompletionRenderer::new(self.analysis, self.query);
        for method in completion_view.method_candidates_for_dot(&site)? {
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
