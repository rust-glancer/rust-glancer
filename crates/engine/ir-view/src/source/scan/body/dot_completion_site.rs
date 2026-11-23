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

use rg_body_ir::{BodyReader, BodyView, ExprData, ExprKind};
use rg_ir_model::{BodySource, CrateRef, ExprId, FileId, Span};
use rg_source::ReadError;
use rg_ty::Ty;

use super::DotCompletionSite;

/// Finds the source site that belongs to a dot-completion offset.
///
/// The result identifies the lowered receiver separately from the source span to replace. Candidate
/// generation can therefore inspect the receiver type without having to rediscover dot syntax.
pub(crate) struct DotCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyReader<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> DotCompletionSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyReader<'db>,
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

    /// Returns the narrowest access at the cursor, keeping distinct copies at the same site.
    pub(crate) fn sites_at_dot(&self) -> Result<Vec<DotCompletionSite>, ReadError> {
        let mut best: Vec<DotCandidate<'_>> = Vec::new();

        for (body_ref, body) in
            self.body_ir
                .bodies_at(self.crate_ref, self.file_id, self.offset, None)?
        {
            // An unfinished body can end exactly at the cursor. Completion therefore accepts the
            // closed end of the body span in addition to ordinary offsets inside it.
            if !body.source().span.touches(self.offset) {
                continue;
            }

            for expr in body.exprs().iter() {
                let source = body.source_at(expr.source);
                if !source.is_exact_in_file(self.file_id) {
                    continue;
                }
                let Some((receiver, receiver_data, dot)) = Self::receiver_expr(expr)
                    .and_then(|id| Some((id, body.expr(id)?, Self::dot_span(expr, body)?)))
                else {
                    continue;
                };
                let receiver_source = body.source_at(receiver_data.source);
                let receiver_span = if source.is_written() {
                    if !receiver_source.is_written_in_file(self.file_id) {
                        continue;
                    }
                    receiver_source.span
                } else {
                    // In `forward!(make!().re)`, the lowered receiver describes the result
                    // of `make!()`. The contiguous access still proves its written prefix.
                    Span {
                        start: source.span.start,
                        end: dot.start,
                    }
                };
                let Some(member_prefix_span) =
                    self.member_prefix_span(expr, body, receiver_span, dot, source.span.end)
                else {
                    continue;
                };
                if let Some(first) = best.first() {
                    if source.span.len() < first.source.span.len() {
                        best.clear();
                    } else if source.span.len() > first.source.span.len()
                        || (source.is_written() && first.source.is_written())
                    {
                        continue;
                    }
                }
                let receiver_ty = body.expr_ty(receiver);
                if best.iter().any(|other| {
                    other.site.body == body_ref
                        && other.site.scope == expr.scope
                        && other.site.receiver_span == receiver_span
                        && other.site.member_prefix_span == member_prefix_span
                        && other.receiver_ty == receiver_ty
                }) {
                    continue;
                }
                best.push(DotCandidate {
                    site: DotCompletionSite {
                        body: body_ref,
                        receiver,
                        scope: expr.scope,
                        receiver_span,
                        member_prefix_span,
                    },
                    source,
                    receiver_ty,
                });
            }
        }

        Ok(best.into_iter().map(|candidate| candidate.site).collect())
    }

    /// Returns the already-typed member prefix when `offset` is in this dot expression.
    ///
    /// The accepted range starts after the dot rather than at the beginning of the full expression,
    /// so `receiver$0.field` remains an ordinary cursor query.
    fn member_prefix_span(
        &self,
        expr: &ExprData,
        body: BodyView<'_>,
        receiver: Span,
        dot: Span,
        expr_end: u32,
    ) -> Option<Span> {
        // Accept offsets from just after the dot through the currently typed member name.
        // This covers both `user.$0` and `user.na$0`.
        let offset = self.offset;
        let member_span = Self::member_name_span(expr, body);
        let completion_end = member_span.map_or(expr_end, |span| span.end);

        let offset_matches =
            receiver.end <= dot.start && dot.end <= offset && offset <= completion_end;
        if !offset_matches {
            return None;
        }

        // Parser recovery can attach a later token as the member name for a bare
        // `receiver.`. If the cursor is still between the dot and that token,
        // keep the edit range empty at the cursor so LSP clients can accept it.
        if let Some(member_span) = member_span
            && member_span.start <= offset
        {
            return Some(member_span);
        }

        Some(Span {
            start: offset,
            end: offset,
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
    fn member_name_span(expr: &ExprData, body: BodyView<'_>) -> Option<Span> {
        let name = match &expr.kind {
            ExprKind::MethodCall {
                method_name_location,
                ..
            } => *method_name_location,
            ExprKind::Field { field_location, .. } => *field_location,
            _ => None,
        }?;
        let source = body.source_at(name);
        source.location().is_exact().then_some(source.span)
    }

    /// Returns the source span of the dot token for supported dot-access expressions.
    fn dot_span(expr: &ExprData, body: BodyView<'_>) -> Option<Span> {
        let dot = match &expr.kind {
            ExprKind::MethodCall { dot_location, .. } | ExprKind::Field { dot_location, .. } => {
                *dot_location
            }
            _ => None,
        }?;
        let source = body.source_at(dot);
        source.location().is_exact().then_some(source.span)
    }
}

struct DotCandidate<'a> {
    site: DotCompletionSite,
    source: BodySource,
    receiver_ty: Option<&'a Ty>,
}
