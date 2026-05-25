//! Record-field completion assembly for struct literals and record patterns.

use rg_body_ir::{FieldKey, RecordFieldCompletionSite};

use crate::{
    Analysis,
    api::view::member::MemberView,
    model::{CompletionEdit, CompletionItem},
};

use super::field::FieldCompletionRenderer;

pub(super) struct RecordFieldCompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> RecordFieldCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collects named fields for a record site like `User { na$0 }`.
    pub(super) fn completions(
        &self,
        site: RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = Vec::new();
        let members = MemberView::new(self.0);
        let renderer = FieldCompletionRenderer::new(self.0);

        for field in
            members.field_candidates_for_body_type_path(site.body, site.scope, &site.owner)?
        {
            let Some(completion) = renderer.completion(field, edit) else {
                continue;
            };
            if !matches!(completion.key, FieldKey::Named(_)) {
                continue;
            }
            if site
                .existing_fields
                .iter()
                .any(|existing| existing == &completion.key)
            {
                continue;
            }
            if completions.iter().any(|existing: &CompletionItem| {
                existing.target == completion.item.target && existing.label == completion.item.label
            }) {
                continue;
            }

            completions.push(completion.item);
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
