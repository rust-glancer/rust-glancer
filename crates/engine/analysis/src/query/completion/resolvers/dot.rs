//! Fields, methods, and postfix transforms after a semantically indexed dot receiver.
//!
//! Field and method rows come from the receiver type and keep declaration identity. Postfix rows
//! reuse the same receiver span and inferred type, but remain synthetic whole-expression
//! transformations. This module combines the three families and removes duplicate member targets.

use anyhow::Context as _;
use rg_ir_view::{display::syntax::SyntaxRenderer, member::MemberView};

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionItem, CompletionTarget},
    query::completion::site::DotCompletionSite,
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, FieldCompletionRenderer,
        FunctionCompletionRenderer, FunctionCompletionRequest,
    },
    syntax::CompletionSyntaxContext,
};
use super::postfix::PostfixCompletionResolver;

/// Combines the three result families that can follow a dot.
///
/// Fields and methods retain declaration identity. Postfix rows reuse the same receiver, but
/// replace the whole expression with a synthetic transform such as `value.if` or `value.box`.
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
        syntax_context: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let completion_candidates = CompletionCandidateSource::new(self.analysis.view_db());
        let members = MemberView::new(self.analysis.view_db());
        let mut completions = Vec::new();

        let syntax = SyntaxRenderer::new(
            self.analysis
                .view_db()
                .crate_edition(self.query.crate_ref)
                .context("read dot completion edition")?,
        );
        let field_renderer = FieldCompletionRenderer::new(syntax);
        for field_ref in completion_candidates
            .field_candidates_for_dot(&site)
            .context("collect dot field candidates")?
        {
            let Some(field) = members
                .field(field_ref)
                .context("read dot completion field")?
            else {
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

        let function_renderer = FunctionCompletionRenderer::new(self.query, syntax);
        for method in completion_candidates
            .method_candidates_for_dot(&site)
            .context("collect dot method candidates")?
        {
            let Some(function) = members
                .function(method.function_ref())
                .context("read dot completion method")?
            else {
                continue;
            };
            let target = CompletionTarget::Function(function.function_ref());
            let completion = function_renderer.completion(FunctionCompletionRequest {
                function,
                label_override: None,
                kind: method.kind(),
                applicability: method.applicability(),
                edit,
                call_completion: CallCompletionKind::MethodCall,
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

        // Postfix transformations are an overlay over this source shape. They use the receiver's
        // semantic type, but retain their own synthetic candidate and whole-expression renderer.
        completions.extend(
            PostfixCompletionResolver::new(self.analysis, self.query)
                .completions(&site, syntax_context)
                .context("collect postfix completions")?,
        );

        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
