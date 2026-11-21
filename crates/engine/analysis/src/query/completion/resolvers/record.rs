//! Named-field completion for struct/enum-variant literals and record patterns.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FieldKey};
use rg_ir_view::{
    IndexedViewDb, display::syntax::SyntaxRenderer, member::MemberView, source::IndexedRecordOwner,
};

use crate::{
    model::{CompletionEdit, CompletionItem},
    query::completion::{render::FieldCompletionRenderer, site::RecordFieldCompletionSite},
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
        let site = site.source();
        let use_crate = site.scope().body_ir().crate_ref;
        let is_available = |key: Option<&FieldKey>| {
            key.is_some_and(|key| {
                matches!(key, FieldKey::Named(_)) && !site.existing_fields().contains(key)
            })
        };
        let members = MemberView::new(self.db);
        let renderer = FieldCompletionRenderer::new(SyntaxRenderer::new(
            self.db
                .crate_edition(self.crate_ref)
                .context("read record completion edition")?,
        ));
        let mut completions = Vec::new();

        match site.owner() {
            IndexedRecordOwner::Type(owner) => {
                for field in members
                    .field_candidates_for_type_def(owner)
                    .context("read record type fields")?
                {
                    rg_std::check_cancel!(self.db, "record field completion");
                    if is_available(field.key()) && field.is_suggestible_from(use_crate) {
                        completions.extend(renderer.completion(field, edit));
                    }
                }
            }
            IndexedRecordOwner::EnumVariant(owner) => {
                for field in members
                    .field_candidates_for_enum_variant(owner)
                    .context("read record variant fields")?
                {
                    rg_std::check_cancel!(self.db, "record field completion");
                    if is_available(field.key()) && field.is_suggestible_from(use_crate) {
                        completions.extend(renderer.enum_variant_completion(field, edit));
                    }
                }
            }
        }

        Ok(completions)
    }
}
