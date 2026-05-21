//! Record-field completion site scanning.
//!
//! Record-field sites are the field-name slots inside struct literals and record patterns, such
//! as `User { na$0 }` or `let User { na$0 } = user`.

use rg_def_map::TargetRef;
use rg_item_tree::FieldKey;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    BodyData, BodyId, BodyIrReadTxn, BodyPath, BodyRef, ExprKind, PatData, PatKind,
    RecordExprField, RecordPatField, ScopeId,
};

use super::{super::RecordFieldCompletionSite, sites::BodyScanSites};

/// Finds the record field-list site that belongs to a completion offset.
pub(crate) struct RecordFieldCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> RecordFieldCompletionSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            target,
            file_id,
            offset,
        }
    }

    /// Returns the smallest record field list that accepts completion here.
    pub(crate) fn site_at_record_field(
        &self,
    ) -> Result<Option<RecordFieldCompletionSite>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(RecordFieldCompletionSite, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            self.scan_record_exprs(body_ref, body, &mut best);
            self.scan_record_pats(body_ref, body, &mut best);
        }

        Ok(best.map(|(site, _)| site))
    }

    /// Scans record expressions like `User { na$0: value }`.
    fn scan_record_exprs(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(RecordFieldCompletionSite, u32)>,
    ) {
        for expr in body.exprs.iter() {
            if expr.source.file_id != self.file_id {
                continue;
            }
            let ExprKind::Record {
                path: Some(path),
                field_list_span: Some(field_list_span),
                fields,
                spread,
            } = &expr.kind
            else {
                continue;
            };
            let spread_span = spread.as_ref().map(|spread| spread.source_span);
            let Some(site) = self.site_for_record_fields(
                body_ref,
                expr.scope,
                path,
                *field_list_span,
                fields.iter().map(RecordFieldSpan::from_expr_field),
                spread_span,
            ) else {
                continue;
            };
            Self::remember_site(site, expr.source.span.len(), best);
        }
    }

    /// Scans record patterns like `let User { na$0 } = user`.
    fn scan_record_pats(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(RecordFieldCompletionSite, u32)>,
    ) {
        let sites = BodyScanSites::new(body);
        sites.walk_pats(Some(self.file_id), Some(self.offset), |site| {
            self.scan_pat_data(body_ref, body, site.scope, site.data, best);
        });
    }

    /// Visits record field lists directly owned by one pattern node.
    fn scan_pat_data(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        scope: ScopeId,
        data: &PatData,
        best: &mut Option<(RecordFieldCompletionSite, u32)>,
    ) {
        let PatKind::Record {
            path: Some(path),
            field_list_span: Some(field_list_span),
            fields,
            rest,
        } = &data.kind
        else {
            return;
        };

        let rest_span = rest.and_then(|rest| body.pat(rest).map(|pat| pat.source.span));
        if let Some(site) = self.site_for_record_fields(
            body_ref,
            scope,
            path,
            *field_list_span,
            fields.iter().map(RecordFieldSpan::from_pat_field),
            rest_span,
        ) {
            Self::remember_site(site, data.source.span.len(), best);
        }
    }

    fn site_for_record_fields<'field>(
        &self,
        body: BodyRef,
        scope: ScopeId,
        owner: &BodyPath,
        field_list_span: Span,
        fields: impl Iterator<Item = RecordFieldSpan<'field>>,
        blocked_span: Option<Span>,
    ) -> Option<RecordFieldCompletionSite> {
        if !field_list_span.touches(self.offset) {
            return None;
        }

        let fields = fields.collect::<Vec<_>>();
        let member_prefix_span =
            self.member_prefix_span_for_record_fields(&fields, blocked_span)?;
        let existing_fields = fields.iter().map(|field| field.key.clone()).collect();
        let owner = owner.as_def_map_path()?;

        Some(RecordFieldCompletionSite {
            body,
            scope,
            owner,
            member_prefix_span,
            existing_fields,
        })
    }

    /// Returns the field-name replacement span, while leaving field values to value completion.
    fn member_prefix_span_for_record_fields(
        &self,
        fields: &[RecordFieldSpan<'_>],
        blocked_span: Option<Span>,
    ) -> Option<Span> {
        for field in fields {
            if field.key_span.touches(self.offset) {
                return Some(field.key_span);
            }
            if field.source_span.touches(self.offset) {
                return None;
            }
        }

        if blocked_span.is_some_and(|span| span.touches(self.offset)) {
            return None;
        }

        Some(Span {
            text: TextSpan {
                start: self.offset,
                end: self.offset,
            },
        })
    }

    /// Keeps nested record behavior predictable by choosing the smallest matching record syntax.
    fn remember_site(
        site: RecordFieldCompletionSite,
        source_len: u32,
        best: &mut Option<(RecordFieldCompletionSite, u32)>,
    ) {
        if best
            .as_ref()
            .is_none_or(|(_, best_len)| source_len < *best_len)
        {
            *best = Some((site, source_len));
        }
    }
}

/// Field spans shared by record expressions and record patterns.
struct RecordFieldSpan<'field> {
    key: &'field FieldKey,
    key_span: Span,
    source_span: Span,
}

impl<'field> RecordFieldSpan<'field> {
    fn from_expr_field(field: &'field RecordExprField) -> Self {
        Self {
            key: &field.key,
            key_span: field.key_span,
            source_span: field.source_span,
        }
    }

    fn from_pat_field(field: &'field RecordPatField) -> Self {
        Self {
            key: &field.key,
            key_span: field.key_span,
            source_span: field.source_span,
        }
    }
}
