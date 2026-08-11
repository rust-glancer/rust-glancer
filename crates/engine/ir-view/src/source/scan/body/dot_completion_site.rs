//! Dot-completion site scanning over Body IR.
//!
//! Dot completion scans recognize field and method access expressions that can host member
//! completions. They retain the semantic receiver expression, its exact written span, and the
//! typed member prefix. The span lets request-local postfix syntax verify that it is extending the
//! same receiver before proposing a whole-expression edit.
//!
//! ```text
//! user.$0       empty replacement span after the dot
//! user.na$0     replace the written `na` prefix
//! user.name($0) not a dot-completion site: the cursor is inside the arguments
//! ```

use rg_ir_model::{CrateRef, ExprId};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use rg_body_ir::{BodyIrReadTxn, BodyView, ExprData, ExprKind};

use super::super::NarrowestSourceSite;
use super::DotCompletionSite;

/// Finds the source site that belongs to a dot-completion offset.
///
/// The result identifies the lowered receiver separately from the source span to replace. Candidate
/// generation can therefore inspect the receiver type without having to rediscover dot syntax.
pub(crate) struct DotCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> DotCompletionSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Returns the smallest field or method expression that accepts completions at the dot.
    pub(crate) fn site_at_dot(&self) -> Result<Option<DotCompletionSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();

        for (body_ref, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            // An unfinished body can end exactly at the cursor. Completion therefore accepts the
            // closed end of the body span in addition to ordinary offsets inside it.
            if !body.source().span.touches(self.offset) {
                continue;
            }

            for expr in body.exprs().iter() {
                if !expr.source.is_written_in_file(self.file_id) {
                    continue;
                }
                // Then ask the expression-level matcher whether this dot access accepts
                // completions here and which member prefix has already been typed.
                let Some(member_prefix_span) =
                    Self::member_prefix_span_for_dot_expr(expr, body, self.offset)
                else {
                    continue;
                };

                let Some(receiver) = Self::receiver_expr(expr) else {
                    continue;
                };
                let Some(receiver_data) = body.expr(receiver) else {
                    continue;
                };
                if !receiver_data.source.is_written_in_file(self.file_id) {
                    continue;
                }
                let len = expr.source.span.len();
                // Nested accesses can both contain the offset. The shortest expression is the
                // one the user is completing.
                best.consider(
                    DotCompletionSite {
                        body: body_ref,
                        receiver,
                        receiver_span: receiver_data.source.span,
                        member_prefix_span,
                    },
                    len,
                );
            }
        }

        Ok(best.finish())
    }

    /// Returns the already-typed member prefix when `offset` is in this dot expression.
    ///
    /// The accepted range starts after the dot rather than at the beginning of the full expression,
    /// so `receiver$0.field` remains an ordinary cursor query.
    fn member_prefix_span_for_dot_expr(
        expr: &ExprData,
        body: BodyView<'_>,
        offset: u32,
    ) -> Option<Span> {
        // A completion site needs both the receiver and the dot; incomplete or unrelated
        // expressions simply do not participate.
        let receiver = Self::receiver_expr(expr)?;
        let receiver_data = body.expr(receiver)?;
        let dot_span = Self::dot_span(expr)?;

        // Accept offsets from just after the dot through the currently typed member name.
        // This covers both `user.$0` and `user.na$0`.
        let member_span = Self::member_name_span(expr);
        let completion_end = member_span
            .map(|span| span.text.end)
            .unwrap_or(expr.source.span.text.end);

        let offset_matches = receiver_data.source.span.text.end <= dot_span.text.start
            && dot_span.text.end <= offset
            && offset <= completion_end;
        if !offset_matches {
            return None;
        }

        // Parser recovery can attach a later token as the member name for a bare
        // `receiver.`. If the cursor is still between the dot and that token,
        // keep the edit range empty at the cursor so LSP clients can accept it.
        if let Some(member_span) = member_span
            && member_span.text.start <= offset
        {
            return Some(member_span);
        }

        Some(Span {
            text: TextSpan {
                start: offset,
                end: offset,
            },
        })
    }

    /// Extracts the receiver expression from supported dot-access expression kinds.
    fn receiver_expr(expr: &ExprData) -> Option<ExprId> {
        match &expr.kind {
            ExprKind::MethodCall {
                receiver: Some(receiver),
                ..
            }
            | ExprKind::Field {
                base: Some(receiver),
                ..
            } => Some(*receiver),
            _ => None,
        }
    }

    /// Returns the visible member name, if this dot expression already has one.
    fn member_name_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::MethodCall {
                method_name_span, ..
            } => *method_name_span,
            ExprKind::Field { field_span, .. } => *field_span,
            _ => None,
        }
    }

    /// Returns the source span of the dot token for supported dot-access expressions.
    fn dot_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::MethodCall { dot_span, .. } => *dot_span,
            ExprKind::Field { dot_span, .. } => *dot_span,
            _ => None,
        }
    }
}
