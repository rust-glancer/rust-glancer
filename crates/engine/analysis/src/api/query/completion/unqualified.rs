//! Unqualified completion assembly for lexical and import-root positions.

use std::collections::HashSet;

use rg_body_ir::{
    BodyUnqualifiedCompletionCandidate, UnqualifiedCompletionNamespace, UnqualifiedCompletionSite,
};
use rg_def_map::{DefId, DefMapUnqualifiedCompletionSite, ScopeNamespace, VisibleScopeDef};
use rg_semantic_ir::Documentation;

use crate::{
    Analysis,
    api::render::signature::SignatureRenderer,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionMetadata,
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    def_completion_detail,
};

pub(super) struct UnqualifiedCompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> UnqualifiedCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
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

        for candidate in self.0.body_ir.unqualified_completion_candidates(site)? {
            if !filter.accepts_body_candidate(&candidate) {
                continue;
            }
            self.push_body_completion(
                candidate,
                site,
                filter,
                edit,
                &mut hidden,
                &mut completions,
            )?;
        }

        let Some(body) = self.0.body_ir.body_data(site.body)? else {
            return Ok(completions);
        };
        self.push_module_completions(
            body.owner_module(),
            filter,
            edit,
            &hidden,
            VisibleScopeSort::ByOrigin,
            &mut completions,
        )?;

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
            site.module,
            UnqualifiedCompletionFilter::All,
            edit,
            &hidden,
            VisibleScopeSort::General,
            &mut completions,
        )?;

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_body_completion(
        &self,
        candidate: BodyUnqualifiedCompletionCandidate,
        site: UnqualifiedCompletionSite,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        hidden: &mut HashSet<(String, ScopeNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        match candidate {
            BodyUnqualifiedCompletionCandidate::Binding {
                binding,
                label,
                scope_distance,
            } => {
                hidden.insert((label.clone(), ScopeNamespace::Values));
                let Some(body) = self.0.body_ir.body_data(site.body)? else {
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
                    detail: Some(SignatureRenderer::new(self.0).binding_signature(data)?),
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
                hidden.insert((label.clone(), ScopeNamespace::Types));
                let Some(body) = self.0.body_ir.body_data(item.body)? else {
                    return Ok(());
                };
                let Some(data) = body.local_item(item.item) else {
                    return Ok(());
                };
                let kind = CompletionKind::from_body_item_kind(kind);
                let target = CompletionTarget::BodyItem(item);
                completions.push(CompletionItem {
                    label: label.clone(),
                    kind,
                    target,
                    applicability: CompletionApplicability::Known,
                    detail: Some(SignatureRenderer::new(self.0).local_item_signature(data)),
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
        }
        Ok(())
    }

    fn push_module_completions(
        &self,
        module: rg_def_map::ModuleRef,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        hidden: &HashSet<(String, ScopeNamespace)>,
        visible_scope_sort: VisibleScopeSort,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for visible_def in self.0.def_map.visible_unqualified_scope_defs(module)? {
            if !filter.accepts_scope_namespace(visible_def.namespace)
                || hidden.contains(&(visible_def.label.clone(), visible_def.namespace))
            {
                continue;
            }
            self.push_visible_scope_completion(
                visible_def,
                filter,
                edit,
                visible_scope_sort,
                completions,
            )?;
        }
        Ok(())
    }

    fn push_visible_scope_completion(
        &self,
        visible_def: VisibleScopeDef,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        visible_scope_sort: VisibleScopeSort,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some((kind, metadata)) = self.visible_scope_completion_metadata(&visible_def)? else {
            return Ok(());
        };
        let target = CompletionTarget::Def(visible_def.def);
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == metadata.label)
        {
            return Ok(());
        }

        let sort_policy = filter.sort_policy();
        let sort_text = match visible_scope_sort {
            VisibleScopeSort::ByOrigin => sort_policy.sort_text(
                Some(CompletionSortPriority::visible_scope(visible_def.origin)),
                &metadata.label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            VisibleScopeSort::General => sort_policy.sort_text(
                None,
                &metadata.label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
        };

        completions.push(CompletionItem {
            label: metadata.label.clone(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: metadata.detail,
            documentation: metadata.documentation,
            sort_text,
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn visible_scope_completion_metadata(
        &self,
        visible_def: &VisibleScopeDef,
    ) -> anyhow::Result<Option<(CompletionKind, CompletionMetadata)>> {
        let (kind, metadata) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.0.def_map.module(module)? else {
                    return Ok(None);
                };
                (
                    CompletionKind::Module,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(format!("mod {}", visible_def.label)),
                        documentation: data.docs.as_ref().map(Documentation::text),
                    },
                )
            }
            DefId::Local(local_def) => {
                let Some(data) = self.0.def_map.local_def(local_def)? else {
                    return Ok(None);
                };
                let kind = CompletionKind::from_local_def_kind(data.kind);
                (
                    kind,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(def_completion_detail(kind, &visible_def.label)),
                        documentation: None,
                    },
                )
            }
        };

        Ok(Some((kind, metadata)))
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

impl UnqualifiedCompletionFilter {
    fn accepts_scope_namespace(self, namespace: ScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, ScopeNamespace::Types),
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
