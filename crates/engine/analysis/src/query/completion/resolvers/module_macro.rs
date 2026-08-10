//! Macro-call completion in module and associated-item lists.
//!
//! A bare item prefix can be either an incomplete keyword or a macro name whose `!` has not been
//! typed. Request syntax retains an optional qualifier and item-list context, while the indexed
//! source site supplies the containing semantic module from which macro names resolve. The parent
//! resolver merges item keywords when both interpretations remain valid.

use anyhow::Context as _;

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionItem},
    query::completion::site::ModuleMacroCompletionSite,
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, DefinitionCompletionRenderer,
        DefinitionCompletionRequest,
    },
};

/// Restricts an item-position name lookup to macros that can be invoked with `!`.
pub(super) struct ModuleMacroCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> ModuleMacroCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Complete invocation macros at the indexed module or associated-item owner.
    pub(super) fn completions(
        &self,
        site: ModuleMacroCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let source = CompletionCandidateSource::new(self.analysis.view_db());
        let candidates = source
            .module_macro_candidates(&site)
            .context("collect module macro candidates")?;
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create module macro renderer")?;
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let mut completions = Vec::new();

        for candidate in candidates {
            let Some(completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion: CallCompletionKind::Call,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: None,
                })
                .context("render module macro completion")?
            else {
                continue;
            };
            completions.push(completion);
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
