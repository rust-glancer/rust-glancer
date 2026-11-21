//! Record field candidate lookup.

use anyhow::Context as _;
use rg_ir_model::FieldKey;
use rg_ir_view::{member::MemberView, source::IndexedRecordOwner};

use super::{CompletionCandidateSource, RecordFieldCompletionCandidate};
use crate::query::completion::site::RecordFieldCompletionSite;

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Return named fields owned by the record, excluding keys already present beside the cursor.
    pub(crate) fn field_candidates_for_record(
        &self,
        site: &RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<RecordFieldCompletionCandidate>> {
        let site = site.source();
        let use_crate = site.scope().body_ir().crate_ref;
        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        match site.owner() {
            IndexedRecordOwner::Type(owner) => {
                for field in members
                    .field_candidates_for_type_def(owner)
                    .context("collect type record field candidates")?
                {
                    rg_std::check_cancel!(self.db, "completion candidate");
                    let Some(key) = field.key() else {
                        continue;
                    };
                    if !field.is_suggestible_from(use_crate)
                        || !Self::record_field_is_available(key, site.existing_fields())
                    {
                        continue;
                    }
                    fields.push(RecordFieldCompletionCandidate::Type(field.field_ref()));
                }
            }
            IndexedRecordOwner::EnumVariant(owner) => {
                for field in members
                    .field_candidates_for_enum_variant(owner)
                    .context("collect enum variant record field candidates")?
                {
                    rg_std::check_cancel!(self.db, "completion candidate");
                    let Some(key) = field.key() else {
                        continue;
                    };
                    if !field.is_suggestible_from(use_crate)
                        || !Self::record_field_is_available(key, site.existing_fields())
                    {
                        continue;
                    }
                    fields.push(RecordFieldCompletionCandidate::EnumVariant(
                        field.field_ref(),
                    ));
                }
            }
        }

        Ok(fields)
    }

    fn record_field_is_available(key: &FieldKey, existing_fields: &[FieldKey]) -> bool {
        matches!(key, FieldKey::Named(_)) && !existing_fields.iter().any(|existing| existing == key)
    }
}
