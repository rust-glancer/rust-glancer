//! Qualified path completion assembly across module and associated-item lookup.
//!
//! ```text
//! crate::api::Us$0          -> names from the resolved `api` module
//! Widget::<u8>::ne$0        -> associated items for `Widget<u8>`
//! <T as Factory>::Out$0     -> items from the selected trait hierarchy
//! ```
//!
//! A written qualifier can have both a module-shaped and type-shaped interpretation while source
//! is incomplete. The source site retains both projections, this resolver asks both candidate
//! domains, and stable target identity removes overlap before insertion policy is applied.

use anyhow::Context as _;
use rg_ir_view::{lookup::name::NameNamespace, ty::TyView};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind, CompletionTarget,
    },
    query::completion::{
        CompletionContext,
        candidates::{CompletionCandidateSource, DefinitionCompletionCandidate},
        pattern::{PatternCandidateRole, PatternCompletionPolicy},
        relevance::CompletionSortPolicy,
        render::{CallCompletionKind, DefinitionCompletionRenderer, DefinitionCompletionRequest},
        site::{NameCompletionContext, PathCompletionSite},
    },
};

/// Merges the module-shaped and type-shaped interpretations of a written path qualifier.
///
/// Both interpretations can be valid while a path is incomplete, as in `Widget::ne$0`. Results
/// are filtered by the surrounding type/value/pattern grammar and collapsed by stable identity.
pub(super) struct PathCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionContext<'source>,
}

impl<'a, 'db, 'source> PathCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionContext<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects qualified path completions, such as `crate::$0` or `use crate::user::$0`.
    pub(super) fn completions(
        &self,
        site: PathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let context = site.context();
        let completion_candidates = CompletionCandidateSource::new(self.analysis.view_db());
        let module_candidates = completion_candidates
            .module_candidates_for_path(&site)
            .context("collect module path completion candidates")?;
        let call_completion = CallCompletionKind::for_name_context(context);
        let associated_candidates = completion_candidates
            .associated_definition_candidates_for_path(
                &site,
                self.query.crate_ref,
                self.query
                    .expected_type
                    .filter(|_| self.query.completes_call(call_completion))
                    .and_then(|ranking| ranking.call_result_type()),
            )
            .context("collect associated path completion candidates")?;
        // Associated lookup can confirm a return type using the qualifier's bindings. Functions
        // from a module path are ranked with the other free functions after rendering.
        let candidates = module_candidates
            .into_iter()
            .map(|candidate| (candidate, false))
            .chain(associated_candidates);
        self.render_path_completions(
            &completion_candidates,
            candidates,
            edit,
            context,
            call_completion,
        )
        .context("render path completions")
    }

    /// Renders the definitions that fit the context after resolving the written qualifier.
    fn render_path_completions(
        &self,
        source: &CompletionCandidateSource<'_, '_>,
        candidates: impl Iterator<Item = (DefinitionCompletionCandidate, bool)>,
        edit: CompletionEdit,
        context: NameCompletionContext,
        call_completion: CallCompletionKind,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create path completion renderer")?;
        let types = TyView::new(self.analysis.view_db());
        let mut completions: Vec<CompletionItem> = Vec::new();
        let pattern_policy = match context {
            NameCompletionContext::Pattern(kind) => Some(PatternCompletionPolicy::new(
                kind,
                self.query.client_capabilities.snippet_support,
            )),
            NameCompletionContext::Type
            | NameCompletionContext::Value
            | NameCompletionContext::Const
            | NameCompletionContext::Import => None,
        };

        for (candidate, return_type_matches) in candidates {
            rg_std::check_cancel!(self.analysis, "completion candidate");
            if pattern_policy.is_some()
                && candidate.kind() == CompletionKind::Macro
                && !candidate.is_invocation_macro()
            {
                continue;
            }
            let pattern_role = if let Some(policy) = pattern_policy {
                let shape = source
                    .pattern_constructor_shape(candidate.target())
                    .context("read path pattern constructor shape")?;
                let Some(role) =
                    policy.candidate(candidate.kind(), Some(candidate.namespace()), shape)
                else {
                    continue;
                };
                Some(role)
            } else {
                let filter = PathCompletionFilter::from(context);
                if !filter.accepts(candidate.namespace(), candidate.kind()) {
                    continue;
                }
                None
            };
            let Some(mut completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: None,
                })
                .context("render path definition completion")?
            else {
                continue;
            };
            if let (Some(policy), Some(PatternCandidateRole::Constructor(shape))) =
                (pattern_policy, pattern_role)
            {
                completion.insert_text =
                    policy.constructor_insert_text(&completion.label, &shape, true);
            }
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            if context == NameCompletionContext::Value
                && completion.applicability == CompletionApplicability::Known
                && let CompletionTarget::Function(function) = completion.target
                && let Some(arity) = types.constructor_arity(function)?
            {
                completion.relevance.set_constructor_priority(arity > 0);
            }
            if return_type_matches && let Some(ranking) = self.query.expected_type {
                ranking.promote_call_match(&mut completion);
            }
            completions.push(completion);
        }

        if self.query.completes_call(call_completion)
            && let Some(ranking) = self.query.expected_type
        {
            ranking.rank_free_functions(types, &mut completions)?;
        }

        Ok(completions)
    }
}

/// Namespace policy for the segment being completed in a qualified path.
///
/// Type positions like `let value: crate::$0` accept type-namespace candidates.
/// Value positions like `let value = crate::$0` accept all candidates so modules
/// and types can still be used as prefixes on the way to a value item such as
/// `crate::user::build_user()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCompletionFilter {
    Types,
    Consts,
    All,
}

impl PathCompletionFilter {
    fn accepts(self, namespace: NameNamespace, kind: CompletionKind) -> bool {
        match self {
            Self::Types => matches!(namespace, NameNamespace::Types),
            Self::Consts => matches!(kind, CompletionKind::Module | CompletionKind::Const),
            Self::All => true,
        }
    }
}

impl From<NameCompletionContext> for PathCompletionFilter {
    fn from(context: NameCompletionContext) -> Self {
        match context {
            NameCompletionContext::Type => Self::Types,
            NameCompletionContext::Const => Self::Consts,
            NameCompletionContext::Value
            | NameCompletionContext::Pattern(_)
            | NameCompletionContext::Import => Self::All,
        }
    }
}
