//! Named-field completion for struct/enum-variant literals and record patterns.

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_ir_view::{IndexedViewDb, display::syntax::SyntaxRenderer, member::MemberView};

use crate::{
    model::{CompletionEdit, CompletionItem},
    query::completion::site::RecordFieldCompletionSite,
};

use super::super::{
    candidates::{CompletionCandidateSource, RecordFieldCompletionCandidate},
    render::FieldCompletionRenderer,
};

/// Turns a resolved record owner into field rows, excluding fields already written in the list.
pub(super) struct RecordFieldCompletionResolver<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    crate_ref: CrateRef,
}

impl<'a, 'db> RecordFieldCompletionResolver<'a, 'db> {
    pub(super) fn new(db: &'a IndexedViewDb<'db>, crate_ref: CrateRef) -> Self {
        Self { db, crate_ref }
    }

    /// Collects named fields for a record site like `User { na$0 }`.
    pub(super) fn completions(
        &self,
        site: RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let completion_candidates = CompletionCandidateSource::new(self.db);
        let members = MemberView::new(self.db);
        let renderer = FieldCompletionRenderer::new(SyntaxRenderer::new(
            self.db
                .crate_edition(self.crate_ref)
                .context("read record completion edition")?,
        ));
        let mut completions = Vec::new();

        for candidate in completion_candidates
            .field_candidates_for_record(&site)
            .context("collect record field candidates")?
        {
            let completion = match candidate {
                RecordFieldCompletionCandidate::Type(field_ref) => {
                    let Some(field) = members
                        .field(field_ref)
                        .context("read record completion field")?
                    else {
                        continue;
                    };
                    renderer.completion(field, edit)
                }
                RecordFieldCompletionCandidate::EnumVariant(field_ref) => {
                    let Some(field) = members
                        .enum_variant_field(field_ref)
                        .context("read enum variant record completion field")?
                    else {
                        continue;
                    };
                    renderer.enum_variant_completion(field, edit)
                }
            };
            let Some(completion) = completion else {
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
