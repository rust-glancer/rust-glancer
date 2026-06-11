//! Shared rendering for module-scope completion candidates.

use rg_ir_view::member::MemberView;

use crate::{
    Analysis,
    model::{CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem},
};

use super::{
    CompletionQuery,
    candidates::ModuleCompletionCandidate,
    def_completion_detail,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
};

use super::completion_sort::{CompletionSortPolicy, CompletionSortPriority};

pub(super) struct ModuleCompletionRequest<'candidate> {
    pub(super) candidate: &'candidate ModuleCompletionCandidate,
    pub(super) edit: CompletionEdit,
    pub(super) function_call_completion: FunctionCallCompletion,
    pub(super) sort_policy: CompletionSortPolicy,
    pub(super) sort_priority: Option<CompletionSortPriority>,
}

pub(super) struct ModuleCompletionRenderer<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> ModuleCompletionRenderer<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    pub(super) fn completion(
        &self,
        request: ModuleCompletionRequest<'_>,
    ) -> anyhow::Result<Option<CompletionItem>> {
        if let Some(function_ref) = request.candidate.function_ref() {
            let members = MemberView::new(self.analysis.view_db());
            let Some(function) = members.function(function_ref)? else {
                return Ok(None);
            };
            return Ok(Some(
                FunctionCompletionRenderer::new(self.query)
                    .completion(FunctionCompletionRequest {
                        function,
                        label_override: Some(request.candidate.label()),
                        kind: request.candidate.kind(),
                        applicability: CompletionApplicability::Known,
                        edit: request.edit,
                        call_completion: request.function_call_completion,
                        sort_policy: request.sort_policy,
                        sort_priority: request.sort_priority,
                    })
                    .item,
            ));
        }

        let target = request.candidate.target();
        let label = request.candidate.label();
        let kind = request.candidate.kind();
        Ok(Some(CompletionItem {
            label: label.to_string(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(kind, label)),
            documentation: request.candidate.documentation().map(ToString::to_string),
            sort_text: request.sort_policy.sort_text(
                request.sort_priority,
                label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(request.edit),
        }))
    }
}
