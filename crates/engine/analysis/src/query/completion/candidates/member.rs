//! Field, record, method, and receiver candidate lookup.

use anyhow::Context as _;
use rg_ir_model::{FieldKey, FieldRef, PrimitiveTy};
use rg_ir_view::{
    member::{MemberMethodCandidate, MemberMethodOrigin, MemberView},
    source::IndexedRecordOwner,
    ty::locals::BodyView,
};

use crate::{
    model::{CompletionApplicability, CompletionKind},
    query::completion::site::{DotCompletionSite, RecordFieldCompletionSite},
};

use super::{
    CompletionCandidateSource, DotMethodCompletionCandidate, RecordFieldCompletionCandidate,
};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Return fields available after the inferred receiver type in `receiver.fi$0`.
    pub(crate) fn field_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<FieldRef>> {
        let receiver = site.source().receiver();
        let Some(receiver_ty) = BodyView::new(self.db)
            .expr_ty(receiver.body_ir(), receiver.expr_id())
            .context("read dot receiver type for fields")?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        for field in members
            .field_candidates_for_ty(receiver.body_ir().crate_ref, &receiver_ty)
            .context("collect dot field candidates")?
        {
            fields.push(field.field_ref());
        }

        Ok(fields)
    }

    /// Return the receiver's inferred primitive type for type-sensitive postfix transforms.
    pub(crate) fn receiver_primitive_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Option<PrimitiveTy>> {
        let receiver = site.source().receiver();
        Ok(BodyView::new(self.db)
            .expr_ty(receiver.body_ir(), receiver.expr_id())
            .context("read dot receiver primitive type")?
            .and_then(|ty| ty.primitive()))
    }

    /// Return named fields owned by the record, excluding keys already present beside the cursor.
    pub(crate) fn field_candidates_for_record(
        &self,
        site: &RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<RecordFieldCompletionCandidate>> {
        let site = site.source();
        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        match site.owner() {
            IndexedRecordOwner::Type(owner) => {
                for field in members
                    .field_candidates_for_type_def(owner)
                    .context("collect type record field candidates")?
                {
                    let Some(key) = field.key() else {
                        continue;
                    };
                    if !Self::record_field_is_available(key, site.existing_fields()) {
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
                    let Some(key) = field.key() else {
                        continue;
                    };
                    if !Self::record_field_is_available(key, site.existing_fields()) {
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

    /// Return applicable inherent and trait methods for the inferred dot receiver.
    pub(crate) fn method_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<DotMethodCompletionCandidate>> {
        let receiver = site.source().receiver();
        let Some(receiver_ty) = BodyView::new(self.db)
            .expr_ty(receiver.body_ir(), receiver.expr_id())
            .context("read dot receiver type for methods")?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut methods = Vec::new();
        for method in members
            .method_candidates_for_ty(site.source().scope(), &receiver_ty)
            .context("collect dot method candidates")?
        {
            methods.push(Self::dot_method_candidate(method));
        }

        Ok(methods)
    }

    fn dot_method_candidate(method: MemberMethodCandidate<'_>) -> DotMethodCompletionCandidate {
        match method.origin() {
            MemberMethodOrigin::Inherent => DotMethodCompletionCandidate {
                function: method.function().function_ref(),
                kind: CompletionKind::InherentMethod,
                applicability: CompletionApplicability::Known,
            },
            MemberMethodOrigin::Trait { applicability } => DotMethodCompletionCandidate {
                function: method.function().function_ref(),
                kind: CompletionKind::TraitMethod,
                applicability: CompletionApplicability::from(applicability),
            },
        }
    }
}
