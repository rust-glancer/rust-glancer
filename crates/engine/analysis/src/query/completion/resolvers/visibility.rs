//! Restricted-visibility paths and extern-crate roots.
//!
//! ```text
//! pub(in crate::api::$0) struct Token;
//! extern crate se$0;
//! ```
//!
//! A visibility path may name only the containing module or one of its ancestors, so ordinary
//! module completion would be too broad. An `extern crate` declaration has the opposite narrow
//! source: only dependency roots from the extern prelude are valid. This resolver keeps both
//! grammar-specific filters outside the general qualified-path flow.

use anyhow::Context as _;

use crate::{
    Analysis,
    model::{
        CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
        SyntheticCompletionTarget,
    },
    query::completion::site::{CompletionSourceAttachment, RestrictedVisibilityCompletionContext},
};

use super::super::{
    CompletionQuery,
    candidates::{CompletionCandidateSource, DefinitionCompletionCandidate},
    render::{
        CallCompletionKind, CompletionSortPolicy, DefinitionCompletionRenderer,
        DefinitionCompletionRequest, SyntheticCompletionCandidate, SyntheticCompletionRenderer,
    },
    syntax::CompletionSyntaxContext,
};

/// Resolves the two narrow declaration grammars that use module-shaped names.
///
/// Restricted visibility walks only legal ancestor modules. `extern crate`, by contrast, reads
/// only dependency roots. Keeping both here prevents ordinary path completion from widening
/// either candidate set.
pub(super) struct VisibilityCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> VisibilityCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Complete path roots or ancestor modules inside `pub(...)`.
    pub(super) fn restricted_visibility_completions(
        &self,
        context: &RestrictedVisibilityCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut keywords = Vec::new();
        if context.qualifier().is_none() {
            if context.allows_in_keyword() {
                keywords.push(
                    SyntheticCompletionCandidate::new(
                        "in",
                        CompletionKind::Keyword,
                        SyntheticCompletionTarget::SpecializedValue,
                    )
                    .with_insert_text(CompletionInsertText::Text("in ".to_string()))
                    .with_detail("restricted visibility keyword in"),
                );
            }
            for label in ["crate", "self", "super"] {
                keywords.push(
                    SyntheticCompletionCandidate::new(
                        label,
                        CompletionKind::Keyword,
                        SyntheticCompletionTarget::SpecializedValue,
                    )
                    .with_detail(format!("visibility path root {label}")),
                );
            }
        }
        let mut completions =
            SyntheticCompletionRenderer::new(prefix.text(), edit).completions(keywords);

        let Some(qualifier) = context.qualifier() else {
            return Ok(completions);
        };
        let Some(source_site) = CompletionSourceAttachment::new(
            self.analysis,
            self.query.crate_ref,
            self.query.file_id,
        )
        .module_site_at(self.query.offset, &syntax.inline_module_path())
        .context("find restricted visibility module")?
        else {
            return Ok(completions);
        };
        let candidates = CompletionCandidateSource::new(self.analysis.view_db())
            .visibility_module_candidates(source_site.module(), qualifier)
            .context("collect restricted visibility candidates")?;
        self.push_definition_completions(candidates, prefix.text(), edit, &mut completions)
            .context("render restricted visibility completions")?;
        Ok(completions)
    }

    /// Complete dependency crate names after `extern crate`.
    pub(super) fn extern_crate_completions(
        &self,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let Some(source_site) = CompletionSourceAttachment::new(
            self.analysis,
            self.query.crate_ref,
            self.query.file_id,
        )
        .module_site_at(self.query.offset, &syntax.inline_module_path())
        .context("find extern crate completion module")?
        else {
            return Ok(Vec::new());
        };
        let candidates = CompletionCandidateSource::new(self.analysis.view_db())
            .extern_crate_candidates(source_site.module())
            .context("collect extern crate candidates")?;
        let mut completions = Vec::new();
        self.push_definition_completions(candidates, prefix.text(), edit, &mut completions)
            .context("render extern crate completions")?;
        Ok(completions)
    }

    fn push_definition_completions(
        &self,
        candidates: Vec<DefinitionCompletionCandidate>,
        prefix: &str,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create visibility completion renderer")?;
        for candidate in candidates {
            if !candidate.label().starts_with(prefix) {
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
                .context("render visibility definition completion")?
            {
                completions.push(completion);
            }
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(())
    }
}
