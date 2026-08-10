//! Associated type binding completion assembly.
//!
//! ```text
//! trait Stream {
//!     type Item;
//!     type Error;
//! }
//!
//! fn load<T: Stream<Item = u8, Er$0 = Failure>>() {}
//! fn choose<T: Stream<It$0>>() {}
//! ```
//!
//! The explicit form offers `Error` and suppresses the already-bound `Item`. Before `=` is typed,
//! `It$0` is also valid as an ordinary type argument, so associated-type rows are an overlay rather
//! than a replacement for normal type completion. Candidate lookup is restricted to associated
//! types from the selected trait hierarchy; functions and consts use different Rust grammar.

use anyhow::Context as _;
use rg_ir_view::source::IndexedAssociatedTypeBindingSite;

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionItem},
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, DefinitionCompletionRenderer,
        DefinitionCompletionRequest,
    },
};

/// Projects unbound associated type names from a trait already selected by source scanning.
///
/// For `Iterator<Item = u8, In$0 = usize>`, the indexed site supplies `Iterator` and the existing
/// `Item` binding; this resolver keeps only the remaining associated type declarations.
pub(super) struct AssociatedTypeBindingCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> AssociatedTypeBindingCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Render unbound associated types for this trait-binding site.
    pub(super) fn completions(
        &self,
        site: IndexedAssociatedTypeBindingSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let source = CompletionCandidateSource::new(self.analysis.view_db());
        let candidates = source
            .associated_type_binding_candidates(&site)
            .context("collect associated type binding candidates")?;
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create associated type binding renderer")?;
        let edit = CompletionEdit {
            replace: site.member_prefix_span(),
        };
        let mut completions = Vec::new();

        for candidate in candidates {
            let Some(completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion: CallCompletionKind::Plain,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: None,
                })
                .context("render associated type binding completion")?
            else {
                continue;
            };
            completions.push(completion);
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
