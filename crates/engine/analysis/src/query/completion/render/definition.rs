//! Shared rendering for definition-shaped completion candidates.
//!
//! Module, associated-item, and auto-import lookup all produce the same candidate vocabulary.
//! This renderer applies identifier escaping, detail/documentation projection, call or macro
//! insertion policy, and the caller-selected sort policy. Function candidates delegate to the
//! richer signature/snippet renderer.

use anyhow::Context as _;
use rg_ir_view::{display::syntax::SyntaxRenderer, member::MemberView};

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind},
};

use super::super::{CompletionQuery, candidates::DefinitionCompletionCandidate};
use super::{
    CallCompletionKind, definition_detail, escape_lsp_snippet_text,
    function::{FunctionCompletionRenderer, FunctionCompletionRequest},
    sort::{CompletionSortPolicy, CompletionSortPriority},
};

/// Site-specific rendering choices for one definition-shaped candidate.
pub(crate) struct DefinitionCompletionRequest<'candidate> {
    pub(crate) candidate: &'candidate DefinitionCompletionCandidate,
    pub(crate) edit: CompletionEdit,
    pub(crate) call_completion: CallCompletionKind,
    pub(crate) sort_policy: CompletionSortPolicy,
    pub(crate) sort_priority: Option<CompletionSortPriority>,
}

/// Renders the common candidate vocabulary without repeating semantic lookup.
pub(crate) struct DefinitionCompletionRenderer<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
    syntax: SyntaxRenderer,
}

impl<'a, 'db, 'source> DefinitionCompletionRenderer<'a, 'db, 'source> {
    pub(crate) fn new(
        analysis: &'a Analysis<'db>,
        query: CompletionQuery<'source>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            analysis,
            query,
            syntax: SyntaxRenderer::new(
                analysis
                    .view_db()
                    .crate_edition(query.crate_ref)
                    .context("read definition completion edition")?,
            ),
        })
    }

    /// Render one definition, delegating function-shaped candidates to the call renderer.
    pub(crate) fn completion(
        &self,
        request: DefinitionCompletionRequest<'_>,
    ) -> anyhow::Result<Option<CompletionItem>> {
        if let Some(function_ref) = request.candidate.function_ref() {
            let members = MemberView::new(self.analysis.view_db());
            let Some(function) = members
                .function(function_ref)
                .context("read definition completion function")?
            else {
                return Ok(None);
            };
            return Ok(Some(
                FunctionCompletionRenderer::new(self.query, self.syntax)
                    .completion(FunctionCompletionRequest {
                        function,
                        label_override: Some(request.candidate.label()),
                        kind: request.candidate.kind(),
                        applicability: request.candidate.applicability(),
                        edit: request.edit,
                        call_completion: request.call_completion,
                        sort_policy: request.sort_policy,
                        sort_priority: request.sort_priority,
                    })
                    .item,
            ));
        }

        let target = request.candidate.target();
        let label = self
            .syntax
            .identifier(request.candidate.label())
            .to_string();
        let kind = request.candidate.kind();
        Ok(Some(CompletionItem {
            label: label.clone(),
            filter_text: None,
            kind,
            target,
            applicability: request.candidate.applicability(),
            detail: Some(definition_detail(kind, &label)),
            documentation: request.candidate.documentation().map(ToString::to_string),
            sort_text: request.sort_policy.sort_text(
                request.sort_priority,
                &label,
                kind,
                request.candidate.applicability(),
                target,
            ),
            insert_text: self.insert_text(kind, &label, request.call_completion, request.edit),
            edit: Some(request.edit),
            additional_edits: Vec::new(),
        }))
    }

    fn insert_text(
        &self,
        kind: CompletionKind,
        label: &str,
        call_completion: CallCompletionKind,
        edit: CompletionEdit,
    ) -> CompletionInsertText {
        // Macros follow the same path-position policy as functions: expression
        // sites get call syntax, while import-like sites keep plain names.
        if !matches!(kind, CompletionKind::Macro)
            || !call_completion.inserts_call_syntax()
            || !self.query.client_capabilities.snippet_support
            || self.macro_invocation_already_present(edit)
        {
            return CompletionInsertText::Plain;
        }

        CompletionInsertText::Snippet(format!("{}!($0)", escape_lsp_snippet_text(label)))
    }

    fn macro_invocation_already_present(&self, edit: CompletionEdit) -> bool {
        let Some(source) = self.query.source_text else {
            return false;
        };
        let Ok(end) = usize::try_from(edit.replace.text.end) else {
            return false;
        };
        let Some(after_completion) = source.get(end..) else {
            return false;
        };

        after_completion.trim_start().starts_with('!')
    }
}
