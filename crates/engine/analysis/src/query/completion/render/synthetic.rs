//! Shared rendering for request-local completion rows without indexed declarations.

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget, SyntheticCompletionTarget,
};

/// One source-adjacent candidate produced by a specialized resolver.
pub(crate) struct SyntheticCompletionCandidate {
    label: String,
    match_text: String,
    kind: CompletionKind,
    target: CompletionTarget,
    insert_text: CompletionInsertText,
    detail: Option<String>,
}

impl SyntheticCompletionCandidate {
    pub(crate) fn new(
        label: impl Into<String>,
        kind: CompletionKind,
        target: SyntheticCompletionTarget,
    ) -> Self {
        let label = label.into();
        Self {
            match_text: label.clone(),
            label,
            kind,
            target: CompletionTarget::Synthetic(target),
            insert_text: CompletionInsertText::Plain,
            detail: None,
        }
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn with_match_text(mut self, match_text: impl Into<String>) -> Self {
        self.match_text = match_text.into();
        self
    }

    pub(crate) fn with_target(mut self, target: CompletionTarget) -> Self {
        self.target = target;
        self
    }

    pub(crate) fn with_insert_text(mut self, insert_text: CompletionInsertText) -> Self {
        self.insert_text = insert_text;
        self
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Filters and renders rows that have no declaration in the indexed stores.
///
/// Examples include `true`, an ABI name, and postfix `box`. Their target records the synthetic
/// family, while this renderer applies prefix matching, edit range, and a stable shared sort band.
pub(crate) struct SyntheticCompletionRenderer<'a> {
    prefix: &'a str,
    edit: CompletionEdit,
}

impl<'a> SyntheticCompletionRenderer<'a> {
    pub(crate) fn new(prefix: &'a str, edit: CompletionEdit) -> Self {
        Self { prefix, edit }
    }

    /// Keep matching rows and turn them into ordinary transport-neutral completion items.
    pub(crate) fn completions(
        &self,
        candidates: impl IntoIterator<Item = SyntheticCompletionCandidate>,
    ) -> Vec<CompletionItem> {
        let mut completions = candidates
            .into_iter()
            .filter(|candidate| candidate.match_text.starts_with(self.prefix))
            .map(|candidate| CompletionItem {
                sort_text: format!(
                    "00-specialized:{:02}:{}",
                    candidate.kind.sort_text_rank(),
                    candidate.label
                ),
                label: candidate.label,
                filter_text: None,
                kind: candidate.kind,
                target: candidate.target,
                applicability: CompletionApplicability::Known,
                detail: candidate.detail,
                documentation: None,
                insert_text: candidate.insert_text,
                edit: Some(self.edit),
                additional_edits: Vec::new(),
            })
            .collect::<Vec<_>>();
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        completions
    }
}
