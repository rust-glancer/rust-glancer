//! Dot-completion site scanning.
//!
//! Dot completion site scans recognize field and method access expressions that
//! can host member completions, then return the receiver expression and typed
//! member prefix.

use rg_def_map::TargetRef;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::{BodyData, BodyId, BodyIrReadTxn, BodyRef, ExprData, ExprId, ExprKind};

use super::super::DotCompletionSite;

/// Finds the source site that belongs to a dot-completion offset.
pub(crate) struct DotCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> DotCompletionSiteScanner<'txn, 'db> {
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

    /// Returns the smallest field or method expression that accepts completions at the dot.
    pub(crate) fn site_at_dot(&self) -> Result<Option<DotCompletionSite>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(DotCompletionSite, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            // First narrow the search to bodies that can contain this completion offset.
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            for expr in body.exprs.iter() {
                if expr.source.file_id != self.file_id {
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
                let len = expr.source.span.len();
                // Nested accesses can both contain the offset. The shortest expression is the
                // one the user is completing.
                if best.is_none_or(|(_, best_len)| len < best_len) {
                    best = Some((
                        DotCompletionSite {
                            body: body_ref,
                            receiver,
                            member_prefix_span,
                        },
                        len,
                    ));
                }
            }
        }

        Ok(best.map(|(receiver, _)| receiver))
    }

    /// Returns the already-typed member prefix when `offset` is in this dot expression.
    fn member_prefix_span_for_dot_expr(
        expr: &ExprData,
        body: &BodyData,
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
