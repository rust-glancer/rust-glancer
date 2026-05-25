//! Unqualified completion assembly for lexical and import-root positions.

use std::collections::HashSet;

use rg_body_ir::{
    BodyUnqualifiedCompletionCandidate, ResolvedFunctionRef, UnqualifiedCompletionNamespace,
    UnqualifiedCompletionSite,
};
use rg_def_map::DefMapUnqualifiedCompletionSite;
use rg_semantic_ir::Documentation;

use crate::{
    Analysis,
    api::{
        render::signature::SignatureRenderer,
        view::{
            completion::{CompletionScopeNamespace, CompletionView, ModuleCompletionCandidate},
            member::MemberView,
        },
    },
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionQuery,
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    def_completion_detail,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
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

        for candidate in self
            .analysis
            .body_ir
            .unqualified_completion_candidates(&site)?
        {
            if !filter.accepts_body_candidate(&candidate) {
                continue;
            }
            self.push_body_completion(
                candidate,
                &site,
                filter,
                edit,
                &mut hidden,
                &mut completions,
            )?;
        }

        self.push_module_completions(
            CompletionView::new(self.analysis).module_candidates_for_body_unqualified(&site)?,
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
            completions.extend(PrimitiveTypeCompletionResolver::body_completions(
                self.analysis,
                &site,
                edit,
            )?);
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

    fn push_body_completion(
        &self,
        candidate: BodyUnqualifiedCompletionCandidate,
        site: &UnqualifiedCompletionSite,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        hidden: &mut HashSet<(String, CompletionScopeNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        match candidate {
            BodyUnqualifiedCompletionCandidate::Binding {
                binding,
                label,
                scope_distance,
            } => {
                hidden.insert((label.clone(), CompletionScopeNamespace::Values));
                let Some(body) = self.analysis.body_ir.body_data(site.body)? else {
                    return Ok(());
                };
                let Some(data) = body.binding(binding) else {
                    return Ok(());
                };
                let target = CompletionTarget::Binding {
                    body: site.body,
                    binding,
                };
                completions.push(CompletionItem {
                    label,
                    kind: CompletionKind::Variable,
                    target,
                    applicability: CompletionApplicability::Known,
                    detail: Some(SignatureRenderer::new(self.analysis).binding_signature(data)?),
                    documentation: None,
                    sort_text: filter.sort_policy().sort_text(
                        Some(CompletionSortPriority::body_scope(scope_distance)),
                        data.name.as_deref().unwrap_or("<unsupported>"),
                        CompletionKind::Variable,
                        CompletionApplicability::Known,
                        target,
                    ),
                    insert_text: CompletionInsertText::Plain,
                    edit: Some(edit),
                });
            }
            BodyUnqualifiedCompletionCandidate::LocalItem {
                item,
                kind,
                label,
                scope_distance,
            } => {
                let Some(body) = self.analysis.body_ir.body_data(item.body)? else {
                    return Ok(());
                };
                let Some(data) = body.local_item(item.item) else {
                    return Ok(());
                };
                hidden.insert((label.clone(), CompletionScopeNamespace::Types));
                if matches!(site.namespace, UnqualifiedCompletionNamespace::Values)
                    && data.has_value_constructor()
                {
                    hidden.insert((label.clone(), CompletionScopeNamespace::Values));
                }
                let kind = CompletionKind::from_body_item_kind(kind);
                let target = CompletionTarget::BodyItem(item);
                completions.push(CompletionItem {
                    label: label.clone(),
                    kind,
                    target,
                    applicability: CompletionApplicability::Known,
                    detail: Some(SignatureRenderer::new(self.analysis).local_item_signature(data)),
                    documentation: data.docs.as_ref().map(Documentation::text),
                    sort_text: filter.sort_policy().sort_text(
                        Some(CompletionSortPriority::body_scope(scope_distance)),
                        &label,
                        kind,
                        CompletionApplicability::Known,
                        target,
                    ),
                    insert_text: CompletionInsertText::Plain,
                    edit: Some(edit),
                });
            }
            BodyUnqualifiedCompletionCandidate::LocalValueItem {
                item,
                kind,
                label,
                scope_distance,
            } => {
                hidden.insert((label.clone(), CompletionScopeNamespace::Values));
                let Some(body) = self.analysis.body_ir.body_data(item.body)? else {
                    return Ok(());
                };
                let Some(data) = body.local_value_item(item.item) else {
                    return Ok(());
                };
                let kind = CompletionKind::from_body_value_item_kind(kind);
                let target = CompletionTarget::BodyValueItem(item);
                completions.push(CompletionItem {
                    label: label.clone(),
                    kind,
                    target,
                    applicability: CompletionApplicability::Known,
                    detail: Some(
                        SignatureRenderer::new(self.analysis).local_value_item_signature(data),
                    ),
                    documentation: data.docs.as_ref().map(Documentation::text),
                    sort_text: filter.sort_policy().sort_text(
                        Some(CompletionSortPriority::body_scope(scope_distance)),
                        &label,
                        kind,
                        CompletionApplicability::Known,
                        target,
                    ),
                    insert_text: CompletionInsertText::Plain,
                    edit: Some(edit),
                });
            }
            BodyUnqualifiedCompletionCandidate::LocalFunction {
                function,
                label,
                scope_distance,
            } => {
                hidden.insert((label.clone(), CompletionScopeNamespace::Values));
                let function = ResolvedFunctionRef::BodyLocal(function);
                let members = MemberView::new(self.analysis);
                let Some(function) = members.function(function)? else {
                    return Ok(());
                };
                let completion = FunctionCompletionRenderer::new(self.analysis, self.query)
                    .completion(FunctionCompletionRequest {
                        function,
                        label_override: Some(&label),
                        kind: CompletionKind::Function,
                        applicability: CompletionApplicability::Known,
                        edit,
                        call_completion: FunctionCallCompletion::Plain,
                        sort_policy: filter.sort_policy(),
                        sort_priority: Some(CompletionSortPriority::body_scope(scope_distance)),
                    });
                completions.push(completion.item);
            }
        }
        Ok(())
    }

    fn push_module_completions(
        &self,
        candidates: Vec<ModuleCompletionCandidate>,
        options: ModuleCompletionOptions,
        hidden: &HashSet<(String, CompletionScopeNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for candidate in candidates {
            if !options
                .filter
                .accepts_scope_namespace(candidate.namespace())
                || hidden.contains(&(candidate.label().to_string(), candidate.namespace()))
            {
                continue;
            }
            self.push_module_candidate_completion(
                candidate,
                options.filter,
                options.edit,
                options.visible_scope_sort,
                options.function_call_completion,
                completions,
            )?;
        }
        Ok(())
    }

    fn push_module_candidate_completion(
        &self,
        candidate: ModuleCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        visible_scope_sort: VisibleScopeSort,
        function_call_completion: FunctionCallCompletion,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        if let Some(completion) = self.function_completion(
            &candidate,
            filter,
            edit,
            visible_scope_sort,
            function_call_completion,
        )? {
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                return Ok(());
            }
            completions.push(completion);
            return Ok(());
        }

        let target = candidate.target();
        let label = candidate.label();
        let kind = candidate.kind();
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == label)
        {
            return Ok(());
        }

        let sort_policy = filter.sort_policy();
        let sort_text = match visible_scope_sort {
            VisibleScopeSort::ByOrigin => sort_policy.sort_text(
                Some(CompletionSortPriority::visible_scope(candidate.origin())),
                label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            VisibleScopeSort::General => {
                sort_policy.sort_text(None, label, kind, CompletionApplicability::Known, target)
            }
        };

        completions.push(CompletionItem {
            label: label.to_string(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(kind, label)),
            documentation: candidate.documentation().map(ToString::to_string),
            sort_text,
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn function_completion(
        &self,
        candidate: &ModuleCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        visible_scope_sort: VisibleScopeSort,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Option<CompletionItem>> {
        let Some(function_ref) = candidate.function_ref() else {
            return Ok(None);
        };
        let members = MemberView::new(self.analysis);
        let Some(function) = members.function(function_ref)? else {
            return Ok(None);
        };
        let sort_policy = filter.sort_policy();
        let sort_priority = match visible_scope_sort {
            VisibleScopeSort::ByOrigin => {
                Some(CompletionSortPriority::visible_scope(candidate.origin()))
            }
            VisibleScopeSort::General => None,
        };

        Ok(Some(
            FunctionCompletionRenderer::new(self.analysis, self.query)
                .completion(FunctionCompletionRequest {
                    function,
                    label_override: Some(candidate.label()),
                    kind: CompletionKind::Function,
                    applicability: CompletionApplicability::Known,
                    edit,
                    call_completion: function_call_completion,
                    sort_policy,
                    sort_priority,
                })
                .item,
        ))
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

    fn accepts_body_candidate(self, candidate: &BodyUnqualifiedCompletionCandidate) -> bool {
        match self {
            Self::Types => matches!(
                candidate,
                BodyUnqualifiedCompletionCandidate::LocalItem { .. }
            ),
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
