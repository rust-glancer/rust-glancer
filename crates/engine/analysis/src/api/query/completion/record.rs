//! Record-field completion assembly for struct literals and record patterns.

use rg_body_ir::{BodyTypePathResolution, FieldKey, RecordFieldCompletionSite};

use crate::{
    Analysis,
    api::view::member::{MemberField, MemberOwnerRef, MemberView},
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

        for field in self.fields_for_record_owner(&members, &site)? {
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

    /// Resolves the path before `{ ... }` into fields that can be written inside the record.
    fn fields_for_record_owner<'view>(
        &self,
        members: &'view MemberView<'_, '_>,
        site: &RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let resolution = self.0.body_ir.resolve_type_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            site.body,
            site.scope,
            &site.owner,
        )?;
        let mut fields = Vec::new();

        match resolution {
            BodyTypePathResolution::BodyLocal(item) => {
                fields.extend(members.field_candidates_for_owner(MemberOwnerRef::BodyLocal(item))?);
            }
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                for ty in types {
                    fields
                        .extend(members.field_candidates_for_owner(MemberOwnerRef::Semantic(ty))?);
                }
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => {}
        }

        Ok(fields)
    }
}
