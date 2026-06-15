//! Record-field completion assembly for struct literals and record patterns.

use rg_ir_view::{IndexedViewDb, member::MemberView};

use crate::{
    completion_site::RecordFieldCompletionSite,
    model::{CompletionEdit, CompletionItem},
};

use super::{candidates::CompletionCandidateSource, field::FieldCompletionRenderer};

pub(super) struct RecordFieldCompletionResolver<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> RecordFieldCompletionResolver<'a, 'db> {
    pub(super) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    /// Collects named fields for a record site like `User { na$0 }`.
    pub(super) fn completions(
        &self,
        site: RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let completion_candidates = CompletionCandidateSource::new(self.0);
        let members = MemberView::new(self.0);
        let renderer = FieldCompletionRenderer::new();
        let mut completions = Vec::new();

        for field_ref in completion_candidates.field_candidates_for_record(&site)? {
            let Some(field) = members.field(field_ref)? else {
                continue;
            };
            let Some(completion) = renderer.completion(field, edit) else {
                continue;
            };
            if completions.iter().any(|existing: &CompletionItem| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }

            completions.push(completion);
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
