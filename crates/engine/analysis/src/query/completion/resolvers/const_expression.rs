//! Completion for source-classified const expressions.
//!
//! ```text
//! const LIMIT: usize = 64;
//! struct Buffer<const N: usize>;
//!
//! type Packet = Buffer<{ LIM$0 }>;
//! ```
//!
//! Const arguments use value-shaped lookup, but they cannot accept arbitrary runtime values.
//! Unqualified sites therefore reuse lexical, generic, and module lookup under a const context;
//! qualified module paths retain only modules and constants. Boolean literals are synthetic
//! because they have no declaration identity.

use anyhow::Context as _;
use rg_ir_view::source::{IndexedUnqualifiedNameContext, SourceCompletionView};

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionItem, CompletionKind, SyntheticCompletionTarget},
    query::completion::site::{
        CompletionSourceAttachment, ConstExpressionCompletionContext, UnqualifiedCompletionSite,
    },
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, DefinitionCompletionRenderer,
        DefinitionCompletionRequest, SyntheticCompletionCandidate, SyntheticCompletionRenderer,
    },
    syntax::CompletionSyntaxContext,
};
use super::unqualified::UnqualifiedCompletionResolver;

/// Reuses ordinary name lookup under const-expression restrictions.
///
/// An unqualified site may use lexical consts and generics; a qualified site keeps only modules
/// and associated/module constants. Boolean literals are added as language-owned rows.
pub(super) struct ConstExpressionCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> ConstExpressionCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Complete values allowed by the const-expression grammar at this site.
    pub(super) fn completions(
        &self,
        context: &ConstExpressionCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut completions = SyntheticCompletionRenderer::new(prefix.text(), edit).completions([
            SyntheticCompletionCandidate::new(
                "false",
                CompletionKind::Value,
                SyntheticCompletionTarget::SpecializedValue,
            )
            .with_detail("boolean const value false"),
            SyntheticCompletionCandidate::new(
                "true",
                CompletionKind::Value,
                SyntheticCompletionTarget::SpecializedValue,
            )
            .with_detail("boolean const value true"),
        ]);

        if let Some(qualifier) = context.qualifier() {
            let Some(source_site) = CompletionSourceAttachment::new(
                self.analysis,
                self.query.crate_ref,
                self.query.file_id,
            )
            .module_site_at(self.query.offset, &syntax.inline_module_path())
            .context("find qualified const completion module")?
            else {
                return Ok(completions);
            };
            let candidates = CompletionCandidateSource::new(self.analysis.view_db())
                .module_candidates_at(source_site.module(), Some(qualifier))
                .context("collect qualified const completion candidates")?;
            let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
                .context("create const completion renderer")?;
            for candidate in candidates {
                if !candidate.label().starts_with(prefix.text())
                    || !matches!(
                        candidate.kind(),
                        CompletionKind::Const | CompletionKind::Module
                    )
                {
                    continue;
                }
                if let Some(completion) = renderer
                    .completion(DefinitionCompletionRequest {
                        candidate: &candidate,
                        edit,
                        call_completion: CallCompletionKind::Plain,
                        sort_policy: CompletionSortPolicy::General,
                        sort_priority: None,
                    })
                    .context("render qualified const completion")?
                {
                    completions.push(completion);
                }
            }
            completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
            return Ok(completions);
        }

        let source = SourceCompletionView::new(self.analysis.view_db());
        let site = source
            .body_syntax_name_site_at(
                self.query.crate_ref,
                self.query.file_id,
                self.query.offset,
                IndexedUnqualifiedNameContext::Const,
                prefix.span(),
                prefix.text().to_string(),
            )
            .context("find body const completion site")?
            .or(CompletionSourceAttachment::new(
                self.analysis,
                self.query.crate_ref,
                self.query.file_id,
            )
            .signature_name_site_at(
                self.query.offset,
                IndexedUnqualifiedNameContext::Const,
                prefix.span(),
                prefix.text().to_string(),
            )
            .context("find signature const completion site")?);
        if let Some(site) = site {
            completions.extend(
                UnqualifiedCompletionResolver::new(self.analysis, self.query)
                    .completions(UnqualifiedCompletionSite::new(site), Some(syntax))
                    .context("collect unqualified const completions")?,
            );
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
