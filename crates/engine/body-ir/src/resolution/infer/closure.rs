//! Closure signatures exist before their bodies so callable bounds and pattern bindings share
//! the same parameter and output slots. Capture analysis is outside this inference layer.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ScopeId};
use rg_item_tree::TypeRef;
use rg_semantic_ir::SemanticItemSource;
use rg_ty::solver::TyShape;

use super::{BodyInference, ReturnTarget};
use crate::body::{ClosureKind, ClosureParamData, ExprKind};

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Apply annotations and infer parameter patterns and the body using the prepared signature.
    /// Callable bounds may already have filled some of its parameter and return slots.
    pub(super) fn infer_closure(
        &mut self,
        expr: ExprId,
        scope: ScopeId,
        kind: ClosureKind,
        params: &[ClosureParamData],
        ret_ty: Option<&TypeRef>,
        body: Option<ExprId>,
    ) -> anyhow::Result<()> {
        self.prepare_closure(expr);
        let TyShape::Closure(signature) = self.inference.root_resolved_expr_ty(expr).shape() else {
            unreachable!("closure signature was prepared")
        };
        let is_async = kind == ClosureKind::Async;
        let output = if is_async {
            let TyShape::AsyncCoroutine { output, .. } = signature.ret.shape() else {
                unreachable!("async closure call returns a coroutine")
            };
            output
        } else {
            signature.ret
        };
        for (param, ty) in params.iter().zip(signature.params) {
            if let Some(annotation) = &param.annotation {
                let annotation = self
                    .context
                    .live()
                    .type_ref(scope, annotation, self.inference.table())
                    .context("resolve closure parameter")?;
                self.inference.constrain_infer_tys(&ty, &annotation);
            }
            if let Some(pat) = param.pat {
                self.infer_pattern(pat, scope, &ty)
                    .context("infer closure pattern")?;
            }
        }
        if let Some(annotation) = ret_ty {
            let annotation = self
                .context
                .live()
                .type_ref(scope, annotation, self.inference.table())
                .context("resolve closure result")?;
            self.inference.constrain_infer_tys(&output, &annotation);
        }
        self.with_return_target(
            ReturnTarget {
                expected: output,
                values: is_async.then(Vec::new),
            },
            |inference| {
                inference
                    .infer_optional(body, &output)
                    .context("infer closure body")?;
                if is_async && let Some(body) = body {
                    let fallthrough = inference.inference.expr_slot(body);
                    inference
                        .infer_async_output(fallthrough)
                        .context("infer async closure output")?;
                }
                Ok(())
            },
        )
    }

    /// Give a closure slots before inferring its body. Call inference can do this early so bounds
    /// on the surrounding call can constrain its signature before the parameter patterns are used.
    /// `infer_closure` also prepares standalone closures; an existing signature keeps its slots.
    pub(super) fn prepare_closure(&mut self, expr: ExprId) {
        if let ExprKind::Closure { params, kind, .. } = &self.body.expr_unchecked(expr).kind
            && !matches!(
                (self.inference.root_resolved_expr_ty(expr)).shape(),
                TyShape::Closure(_)
            )
        {
            self.inference
                .set_expr_closure_ty(self.context.body_ref(), expr, params.len(), *kind);
        }
    }
}
