//! Qualified path completion assembly for body and import positions.

use rg_body_ir::{PathCompletionNamespace, PathCompletionSite};
use rg_def_map::DefMapPathCompletionSite;
use rg_parse::Span;

use crate::{
    Analysis,
    api::view::{
        completion::{CompletionScopeNamespace, CompletionView, ModuleCompletionCandidate},
        enum_variant::{EnumVariant, EnumVariantView},
        member::MemberView,
    },
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionQuery,
    completion_sort::CompletionSortPolicy,
    def_completion_detail,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
};

pub(super) struct PathCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> PathCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects qualified-path completions inside a body, such as
    /// `let value = crate::$0` or `let value: crate::api::Us$0`.
    pub(super) fn body_completions(
        &self,
        site: PathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = self.module_path_completions(
            CompletionView::new(self.analysis).module_candidates_for_body_path(&site)?,
            site.member_prefix_span,
            PathCompletionFilter::from(site.namespace),
            FunctionCallCompletion::FunctionCall,
        )?;

        if matches!(site.namespace, PathCompletionNamespace::Values) {
            self.enum_variant_completions(site, edit, &mut completions)?;
            completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        }

        Ok(completions)
    }

    /// Collects qualified import completions, such as `use crate::api::$0`.
    pub(super) fn use_completions(
        &self,
        site: DefMapPathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.module_path_completions(
            CompletionView::new(self.analysis).module_candidates_for_use_path(&site)?,
            site.member_prefix_span,
            PathCompletionFilter::All,
            FunctionCallCompletion::Plain,
        )
    }

    /// Renders definitions visible from a resolved module qualifier.
    fn module_path_completions(
        &self,
        candidates: Vec<ModuleCompletionCandidate>,
        member_prefix_span: Span,
        filter: PathCompletionFilter,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: member_prefix_span,
        };
        let mut completions = Vec::new();

        for candidate in candidates {
            if !filter.accepts(candidate.namespace()) {
                continue;
            }
            self.push_module_candidate_completion(
                candidate,
                edit,
                function_call_completion,
                &mut completions,
            )?;
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Adds enum variants for type-qualified value paths, such as `Action::Sta$0`.
    fn enum_variant_completions(
        &self,
        site: PathCompletionSite,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for variant in EnumVariantView::new(self.analysis).variants_for_body_type_path(
            site.body,
            site.scope,
            &site.qualifier,
        )? {
            self.push_enum_variant_completion(variant, edit, completions);
        }

        Ok(())
    }

    fn push_enum_variant_completion(
        &self,
        variant: EnumVariant<'_>,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) {
        let target = CompletionTarget::EnumVariant(variant.variant_ref());
        let label = variant.name();
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == label)
        {
            return;
        }

        completions.push(CompletionItem {
            label: label.to_string(),
            kind: CompletionKind::EnumVariant,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(CompletionKind::EnumVariant, label)),
            documentation: variant.docs_text(),
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                label,
                CompletionKind::EnumVariant,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
    }

    /// Adds one visible module-scope definition for path completion, such as
    /// `HashMap` in `std::collections::Ha$0`.
    fn push_module_candidate_completion(
        &self,
        candidate: ModuleCompletionCandidate,
        edit: CompletionEdit,
        function_call_completion: FunctionCallCompletion,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        if let Some(completion) =
            self.function_completion(&candidate, edit, function_call_completion)?
        {
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

        completions.push(CompletionItem {
            label: label.to_string(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(kind, label)),
            documentation: candidate.documentation().map(ToString::to_string),
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn function_completion(
        &self,
        candidate: &ModuleCompletionCandidate,
        edit: CompletionEdit,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Option<CompletionItem>> {
        let Some(function_ref) = candidate.function_ref() else {
            return Ok(None);
        };
        let members = MemberView::new(self.analysis);
        let Some(function) = members.function(function_ref)? else {
            return Ok(None);
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
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: None,
                })
                .item,
        ))
    }
}

/// Namespace policy for the segment being completed in a qualified path.
///
/// Type positions like `let value: crate::$0` accept type-namespace candidates.
/// Value positions like `let value = crate::$0` accept all candidates so modules
/// and types can still be used as prefixes on the way to a value item such as
/// `crate::api::build_user()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCompletionFilter {
    Types,
    All,
}

impl PathCompletionFilter {
    fn accepts(self, namespace: CompletionScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, CompletionScopeNamespace::Types),
            Self::All => true,
        }
    }
}

impl From<PathCompletionNamespace> for PathCompletionFilter {
    fn from(namespace: PathCompletionNamespace) -> Self {
        match namespace {
            PathCompletionNamespace::Types => Self::Types,
            PathCompletionNamespace::Values => Self::All,
        }
    }
}
