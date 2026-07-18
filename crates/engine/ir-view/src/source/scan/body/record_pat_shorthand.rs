//! Source helpers for colonless record pattern fields.

use rg_ir_model::items::FieldKey;
use rg_ir_model::{BindingId, PatId};
use rg_parse::{FileId, Span};
use rg_std::UniqueVec;

use rg_body_ir::{BodyView, PatKind, RecordPatField};

use super::sites::BodyScanSites;

/// Binding metadata recovered from a colonless record pattern field.
///
/// For `User { ref name }`, `field_span` covers `ref name`, `pat_span` also covers
/// `ref name`, and `binding_name_span` covers only `name`. Rename uses those spans to choose
/// between `title: ref name` for a field rename and `name: ref title` for a binding rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordPatShorthandBinding {
    pub(super) binding: BindingId,
    pub(super) key: FieldKey,
    pub(super) field_span: Span,
    pub(super) pat_span: Span,
    pub(super) binding_name_span: Span,
}

impl RecordPatShorthandBinding {
    /// Collects each binding introduced by colonless record fields in the selected source area.
    pub(super) fn collect(
        body: BodyView<'_>,
        file_id: Option<FileId>,
        offset: Option<u32>,
    ) -> Vec<Self> {
        let mut bindings = Vec::new();
        let mut seen = UniqueVec::new();
        BodyScanSites::new(body).walk_pats(file_id, offset, |site| {
            let PatKind::Record { fields, .. } = &site.data.kind else {
                return;
            };

            for field in fields {
                let Some(shorthand) = Self::from_field(body, field) else {
                    continue;
                };
                if seen.push(shorthand.binding) {
                    bindings.push(shorthand);
                }
            }
        });
        bindings
    }

    pub(super) fn from_field(body: BodyView<'_>, field: &RecordPatField) -> Option<Self> {
        if field.syntax.is_explicit() {
            return None;
        }

        let binding = Self::binding_in_pat(body, field.pat)?;
        let binding_data = body.binding(binding)?;
        Some(Self {
            binding,
            key: field.key.clone(),
            field_span: field.source_span,
            pat_span: body.pat(field.pat)?.source.span,
            binding_name_span: binding_data.name_span?,
        })
    }

    fn binding_in_pat(body: BodyView<'_>, pat: PatId) -> Option<BindingId> {
        let pat = body.pat(pat)?;
        match &pat.kind {
            PatKind::Binding {
                binding: Some(binding),
                ..
            } => Some(*binding),
            PatKind::Box { pat } => Self::binding_in_pat(body, *pat),
            _ => None,
        }
    }
}
