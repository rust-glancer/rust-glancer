//! Collect source occurrences from selected body worklists.
//!
//! Source scans collect every body-local declaration and reference-like span
//! that can participate in navigation, references, and symbol queries.
//! Unlike a point scan, nothing is discarded merely because a narrower node overlaps it:
//!
//! ```text
//! let value = input;
//! consume(value);
//!     ^     ^ retain both references, as well as the `value` declaration
//! ```

use anyhow::Context as _;
use rg_body_ir::{BodyView, ExprKind};
use rg_ir_model::{BodyKey, BodyRef, ExprId, FileId, LocalDefRef};

use super::{
    BodySourceCandidate,
    declarations::BodyDeclarationScanner,
    paths::{BodyPathSourceScanner, TypePathSourceScanner},
    sites::BodyScanSites,
};
use crate::IndexedViewDb;

/// Scans selected owners and their nested bodies for written source candidates.
///
/// An exact name can come from an argument even when its enclosing node came from a macro
/// template. Invocation fallbacks alone do not represent source occurrences.
pub(crate) struct BodySourceScanner<'txn, 'db> {
    db: &'txn IndexedViewDb<'db>,
    file_id: Option<FileId>,
    owners: &'txn [BodyKey],
}

impl<'txn, 'db> BodySourceScanner<'txn, 'db> {
    pub(crate) fn new(
        db: &'txn IndexedViewDb<'db>,
        file_id: Option<FileId>,
        owners: &'txn [BodyKey],
    ) -> Self {
        Self {
            db,
            file_id,
            owners,
        }
    }

    /// Expansion has already resolved each call, so only matching calls need source locations.
    pub(crate) fn macro_calls_to(
        &self,
        definitions: &[LocalDefRef],
    ) -> anyhow::Result<Vec<BodySourceCandidate>> {
        let mut candidates = Vec::new();
        for &owner in self.owners {
            rg_std::check_cancel!(self.db, "body macro references");
            for (body, call) in self.db.body_ir.macro_calls(owner)? {
                rg_std::check_cancel!(self.db, "body occurrence candidates");
                if definitions.contains(&call.definition)
                    && let Some(sources) = self.db.body_ir.body_sources(body)?
                {
                    candidates.extend(BodySourceCandidate::macro_call(
                        body,
                        call,
                        sources,
                        self.file_id,
                    ));
                }
            }
        }
        Ok(candidates)
    }

    /// Returns candidates from the selected worklists, optionally limited to one file.
    pub(crate) fn scan(&self) -> anyhow::Result<Vec<BodySourceCandidate>> {
        let mut candidates = Vec::new();
        rg_std::check_cancel!(self.db, "body scan inventory");
        for (body_ref, body) in self.db.body_ir.bodies_for_owners(self.owners)? {
            rg_std::check_cancel!(self.db, "body scan");

            BodyDeclarationScanner::new(self.db, body_ref, body, self.file_id, None)
                .scan(|candidate| candidates.push(candidate))
                .context("collect body declaration occurrences")?;
            for call in body.macro_calls() {
                rg_std::check_cancel!(self.db, "body occurrence candidates");
                candidates.extend(BodySourceCandidate::macro_call(
                    body_ref,
                    call,
                    body.sources(),
                    self.file_id,
                ));
            }
            self.push_expression_reference_candidates(body_ref, body, &mut candidates)
                .context("collect body occurrences")?;
            self.push_record_field_key_candidates(body_ref, body, &mut candidates)
                .context("collect body occurrences")?;

            TypePathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates).scan();
            BodyPathSourceScanner::in_crate(body_ref, body, self.file_id, &mut candidates).scan();
        }
        Ok(candidates)
    }

    /// Adds the exact name span for expressions that resolve as one semantic reference.
    ///
    /// This selects `value` in a path, `push` in `items.push(value)`, `name` in `user.name`, and
    /// `User` in `model::User { name }` instead of indexing each expression's full source range.
    fn push_expression_reference_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        let sites = BodyScanSites::new(body);
        let record_shorthand_values = sites.record_expr_shorthand_value_ids();
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            let expr_id = ExprId(expr_idx);
            // The path scanner owns qualified paths. Record shorthand has its own source
            // candidate so a field and its implicit value can retain separate rename surfaces.
            if matches!(&expr.kind, ExprKind::Path { path }
                if path.segment_count() != 1 || path.as_def_map_path().is_none())
                || record_shorthand_values.contains(&expr_id)
            {
                continue;
            }
            let Some(name) = sites.expression_name(expr) else {
                continue;
            };
            let location = name.location();
            if !self.file_matches(name.file_id) || !location.is_exact() {
                continue;
            }
            candidates.push(BodySourceCandidate::Expr {
                body: body_ref,
                expr: expr_id,
                file_id: name.file_id,
                location,
            });
        }
        Ok(())
    }

    /// Adds record field keys that resolve through their record owner type.
    fn push_record_field_key_candidates(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> anyhow::Result<()> {
        let sites = BodyScanSites::new(body);
        for expr in body.exprs() {
            rg_std::check_cancel!(self.db, "body occurrence candidates");
            if self.file_matches(body.source_at(expr.source).file_id) {
                sites.walk_record_expr_keys(body_ref, expr, None, |candidate| {
                    candidates.push(candidate);
                });
            }
        }
        sites.walk_record_pat_keys(body_ref, self.file_id, None, |candidate| {
            candidates.push(candidate);
        });
        Ok(())
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }
}
