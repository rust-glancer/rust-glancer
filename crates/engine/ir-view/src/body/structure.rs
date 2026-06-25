//! Higher-level source structure recovered from Body IR facts.

use std::collections::HashMap;

use rg_ir_model::{ExprId, ExprKind, ExprWrapperKind, TargetRef};
use rg_ir_storage::ItemStoreQuery;
use rg_parse::{FileId, Span};
use rg_ty::Ty;

use crate::IndexedViewDb;

/// A body-derived construct whose known source span ends at its closing brace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyClosingBraceBlock {
    file_id: FileId,
    span: Span,
    kind: BodyClosingBraceBlockKind,
}

/// Kind of body-owned block used by closing-brace hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyClosingBraceBlockKind {
    Function {
        name: String,
    },
    Match {
        scrutinee: Option<Span>,
    },
    Loop,
    While {
        condition: Option<Span>,
    },
    For {
        pat: Option<Span>,
        iterable: Option<Span>,
    },
}

impl BodyClosingBraceBlock {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &BodyClosingBraceBlockKind {
        &self.kind
    }
}

/// A typed method call that feeds another method segment in a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodChainExprTy {
    file_id: FileId,
    span: Span,
    parent_dot_span: Span,
    ty: Ty,
}

impl MethodChainExprTy {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn parent_dot_span(&self) -> Span {
        self.parent_dot_span
    }

    pub fn ty(&self) -> &Ty {
        &self.ty
    }
}

/// Projects structural facts from body expressions.
pub struct BodyStructureView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyStructureView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return known types for method calls that feed another method call.
    pub fn method_chain_expr_tys(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<MethodChainExprTy>> {
        let Some(target_bodies) = self.db.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut expr_tys = Vec::new();
        for body in target_bodies.bodies() {
            let parent_dot_by_receiver = Self::method_parent_dots_by_receiver(body);

            for (expr_idx, expr) in body.exprs().iter().enumerate() {
                if !expr.source.is_written_in_file(file_id) {
                    continue;
                }
                if !matches!(expr.kind, ExprKind::MethodCall { .. }) {
                    continue;
                }
                let Some(parent_dot_span) = parent_dot_by_receiver.get(&ExprId(expr_idx)).copied()
                else {
                    continue;
                };
                let Some(ty) = body.expr_ty(ExprId(expr_idx)).cloned() else {
                    continue;
                };
                if matches!(ty, Ty::Unknown) {
                    continue;
                }

                expr_tys.push(MethodChainExprTy {
                    file_id: expr.source.file_id,
                    span: expr.source.span,
                    parent_dot_span,
                    ty,
                });
            }
        }

        Ok(expr_tys)
    }

    /// Return body-owned blocks whose source extent ends at their closing brace.
    pub fn closing_brace_blocks(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<BodyClosingBraceBlock>> {
        // Design note:
        // The source span and structural kind are enough for callers to place block-end annotations
        // without reaching back to the body syntax tree that originally produced the facts.

        let Some(target_bodies) = self.db.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let items = ItemStoreQuery::new(self.db);
        let mut blocks = Vec::new();
        for body in target_bodies.bodies() {
            let body_source = body.source();
            if body_source.file_id == file_id
                && let Some(function) = body.function_owner()
                && let Some(function) = items.function_data(function)?
            {
                blocks.push(BodyClosingBraceBlock {
                    file_id: body_source.file_id,
                    span: body_source.span,
                    kind: BodyClosingBraceBlockKind::Function {
                        name: function.name.to_string(),
                    },
                });
            }

            for expr in body.exprs() {
                if !expr.source.is_written_in_file(file_id) {
                    continue;
                }
                let Some(kind) = Self::closing_brace_kind(body, &expr.kind) else {
                    continue;
                };

                blocks.push(BodyClosingBraceBlock {
                    file_id: expr.source.file_id,
                    span: expr.source.span,
                    kind,
                });
            }
        }

        Ok(blocks)
    }

    /// Map a method receiver expression to the parent call's dot span.
    fn method_parent_dots_by_receiver(
        body: &rg_body_ir::ResolvedBodyData,
    ) -> HashMap<ExprId, Span> {
        let mut parent_dot_by_receiver = HashMap::new();
        for expr in body.exprs() {
            let ExprKind::MethodCall {
                receiver: Some(receiver),
                dot_span: Some(dot_span),
                ..
            } = &expr.kind
            else {
                continue;
            };
            let receiver = Self::chain_receiver_base(body, *receiver);
            parent_dot_by_receiver.entry(receiver).or_insert(*dot_span);
        }

        parent_dot_by_receiver
    }

    /// Peel wrappers around a receiver used as the base of a method chain.
    fn chain_receiver_base(body: &rg_body_ir::ResolvedBodyData, receiver: ExprId) -> ExprId {
        let mut current = receiver;
        while let Some(expr) = body.expr(current) {
            let ExprKind::Wrapper {
                kind: ExprWrapperKind::Paren | ExprWrapperKind::Try | ExprWrapperKind::Await,
                inner: Some(inner),
            } = &expr.kind
            else {
                break;
            };
            current = *inner;
        }

        current
    }

    /// Classify a body expression that can produce a closing-brace hint.
    fn closing_brace_kind(
        body: &rg_body_ir::ResolvedBodyData,
        expr: &ExprKind,
    ) -> Option<BodyClosingBraceBlockKind> {
        match expr {
            ExprKind::Match { scrutinee, .. } => Some(BodyClosingBraceBlockKind::Match {
                scrutinee: scrutinee
                    .and_then(|scrutinee| body.expr(scrutinee).map(|expr| expr.source.span)),
            }),
            ExprKind::Loop { .. } => Some(BodyClosingBraceBlockKind::Loop),
            ExprKind::While { condition, .. } => Some(BodyClosingBraceBlockKind::While {
                condition: condition
                    .and_then(|condition| body.expr(condition).map(|expr| expr.source.span)),
            }),
            ExprKind::For { pat, iterable, .. } => Some(BodyClosingBraceBlockKind::For {
                pat: pat.and_then(|pat| body.pat(pat).map(|pat| pat.source.span)),
                iterable: iterable
                    .and_then(|iterable| body.expr(iterable).map(|expr| expr.source.span)),
            }),
            _ => None,
        }
    }
}
