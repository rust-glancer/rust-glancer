//! Unqualified completion assembly for lexical and import-root positions.

use std::collections::HashSet;

use rg_body_ir::{UnqualifiedCompletionNamespace, UnqualifiedCompletionSite};
use rg_def_map::DefMapUnqualifiedCompletionSite;

use crate::{
    Analysis,
    api::view::{
        completion::{
            CompletionScopeNamespace, CompletionView, LexicalCompletionCandidate,
            ModuleCompletionCandidate,
        },
        details::{DeclarationDetailsContext, DeclarationDetailsView},
        member::MemberView,
    },
    model::{CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem},
};

use super::{
    CompletionQuery,
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
    module_scope::{ModuleCompletionRenderer, ModuleCompletionRequest},
    primitive::PrimitiveTypeCompletionResolver,
};

pub(super) struct UnqualifiedCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> UnqualifiedCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects lexical and module-scope completions inside a body, such as
    /// `let value = inp$0` or `let value: Us$0`.
    pub(super) fn body_completions(
        &self,
        site: UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let filter = UnqualifiedCompletionFilter::from(site.namespace);
        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = Vec::new();
        let mut hidden = HashSet::new();

        let completion_view = CompletionView::new(self.analysis);
        for candidate in completion_view.lexical_candidates_for_unqualified(&site)? {
            if !filter.accepts_scope_namespace(candidate.namespace()) {
                continue;
            }
            self.push_lexical_completion(candidate, filter, edit, &mut hidden, &mut completions)?;
        }

        self.push_module_completions(
            completion_view.module_candidates_for_body_unqualified(&site)?,
            ModuleCompletionOptions {
                filter,
                edit,
                visible_scope_sort: VisibleScopeSort::ByOrigin,
                function_call_completion: FunctionCallCompletion::FunctionCall,
            },
            &hidden,
            &mut completions,
        )?;

        if matches!(site.namespace, UnqualifiedCompletionNamespace::Types) {
            completions.extend(PrimitiveTypeCompletionResolver::completions(
                completion_view.primitive_type_candidates_for_unqualified(&site)?,
                edit,
            ));
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Collects import-root completions such as `use st$0`.
    pub(super) fn use_completions(
        &self,
        site: DefMapUnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = Vec::new();
        let hidden = HashSet::new();
        self.push_module_completions(
            CompletionView::new(self.analysis).module_candidates_for_use_unqualified(&site)?,
            ModuleCompletionOptions {
                filter: UnqualifiedCompletionFilter::All,
                edit,
                visible_scope_sort: VisibleScopeSort::General,
                function_call_completion: FunctionCallCompletion::Plain,
            },
            &hidden,
            &mut completions,
        )?;

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_lexical_completion(
        &self,
        candidate: LexicalCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        hidden: &mut HashSet<(String, CompletionScopeNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for namespace in candidate.shadow_namespaces() {
            hidden.insert((candidate.label().to_string(), *namespace));
        }

        if let Some(function_ref) = candidate.function_ref() {
            let members = MemberView::new(self.analysis);
            let Some(function) = members.function(function_ref)? else {
                return Ok(());
            };
            let completion = FunctionCompletionRenderer::new(self.analysis, self.query).completion(
                FunctionCompletionRequest {
                    function,
                    label_override: Some(candidate.label()),
                    kind: candidate.kind(),
                    applicability: CompletionApplicability::Known,
                    edit,
                    call_completion: FunctionCallCompletion::Plain,
                    sort_policy: filter.sort_policy(),
                    sort_priority: Some(CompletionSortPriority::body_scope(
                        candidate.scope_distance(),
                    )),
                },
            );
            completions.push(completion.item);
            return Ok(());
        }

        let Some(declaration_ref) = candidate.declaration_ref() else {
            return Ok(());
        };
        let Some(details) = DeclarationDetailsView::new(self.analysis)
            .details_for_declaration(declaration_ref, &DeclarationDetailsContext::default())?
        else {
            return Ok(());
        };
        let target = candidate.target();
        let kind = candidate.kind();
        completions.push(CompletionItem {
            label: candidate.label().to_string(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: details.signature,
            documentation: details.docs,
            sort_text: filter.sort_policy().sort_text(
                Some(CompletionSortPriority::body_scope(
                    candidate.scope_distance(),
                )),
                candidate.label(),
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn push_module_completions(
        &self,
        candidates: Vec<ModuleCompletionCandidate>,
        options: ModuleCompletionOptions,
        hidden: &HashSet<(String, CompletionScopeNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = ModuleCompletionRenderer::new(self.analysis, self.query);
        for candidate in candidates {
            if !options
                .filter
                .accepts_scope_namespace(candidate.namespace())
                || hidden.contains(&(candidate.label().to_string(), candidate.namespace()))
            {
                continue;
            }

            let Some(completion) = renderer.completion(ModuleCompletionRequest {
                candidate: &candidate,
                edit: options.edit,
                function_call_completion: options.function_call_completion,
                sort_policy: options.filter.sort_policy(),
                sort_priority: match options.visible_scope_sort {
                    VisibleScopeSort::ByOrigin => {
                        Some(CompletionSortPriority::visible_scope(candidate.origin()))
                    }
                    VisibleScopeSort::General => None,
                },
            })?
            else {
                continue;
            };
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnqualifiedCompletionFilter {
    Types,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleScopeSort {
    /// Keep import-root completions in their ordinary global order.
    General,
    /// Rank module-scope names after body-local names but before prelude and extern roots.
    ByOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModuleCompletionOptions {
    filter: UnqualifiedCompletionFilter,
    edit: CompletionEdit,
    visible_scope_sort: VisibleScopeSort,
    function_call_completion: FunctionCallCompletion,
}

impl UnqualifiedCompletionFilter {
    fn accepts_scope_namespace(self, namespace: CompletionScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, CompletionScopeNamespace::Types),
            Self::All => true,
        }
    }

    fn sort_policy(self) -> CompletionSortPolicy {
        match self {
            Self::Types => CompletionSortPolicy::TypePosition,
            Self::All => CompletionSortPolicy::General,
        }
    }
}

impl From<UnqualifiedCompletionNamespace> for UnqualifiedCompletionFilter {
    fn from(namespace: UnqualifiedCompletionNamespace) -> Self {
        match namespace {
            UnqualifiedCompletionNamespace::Types => Self::Types,
            UnqualifiedCompletionNamespace::Values => Self::All,
        }
    }
}
