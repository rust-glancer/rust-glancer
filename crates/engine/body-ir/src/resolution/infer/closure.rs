//! Closure signatures exist before their bodies so callable bounds and pattern bindings share
//! the same parameter and output slots. Capture analysis is outside this inference layer.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ScopeId};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::Ty;

use super::InferenceContext;
use crate::body::{ClosureParamData, ExprKind};

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Apply annotations and infer parameter patterns and the body using the prepared signature.
    /// Callable bounds may already have filled some of its parameter and return slots.
    pub(super) fn infer_closure(
        &mut self,
        expr: ExprId,
        scope: ScopeId,
        params: &[ClosureParamData],
        ret_ty: Option<&TypeRef>,
        body: Option<ExprId>,
    ) -> anyhow::Result<()> {
        self.prepare_closure(expr);
        let Ty::Closure(signature) = self.inference.root_resolved_expr_ty(expr) else {
            unreachable!("closure signature was prepared")
        };
        for (param, ty) in params.iter().zip(&signature.params) {
            if let Some(annotation) = &param.annotation {
                let annotation = self
                    .context
                    .type_refs(scope)
                    .resolve(annotation)
                    .context("resolve closure parameter")?;
                self.inference.constrain_infer_tys(ty, &annotation);
            }
            if let Some(pat) = param.pat {
                self.infer_pattern(pat, ty)
                    .context("infer closure pattern")?;
            }
        }
        if let Some(annotation) = ret_ty {
            let annotation = self
                .context
                .type_refs(scope)
                .resolve(annotation)
                .context("resolve closure result")?;
            self.inference
                .constrain_infer_tys(&signature.ret, &annotation);
        }
        self.infer_optional(body, &signature.ret)
            .context("infer closure body")?;
        Ok(())
    }

    /// Give a closure slots before inferring its body. Call inference can do this early so bounds
    /// on the surrounding call can constrain its signature before the parameter patterns are used.
    /// `infer_closure` also prepares standalone closures; an existing signature keeps its slots.
    pub(super) fn prepare_closure(&mut self, expr: ExprId) {
        if let ExprKind::Closure { params, .. } = &self.body.expr_unchecked(expr).kind
            && !matches!(self.inference.root_resolved_expr_ty(expr), Ty::Closure(_))
        {
            self.inference
                .set_expr_closure_ty(self.context.body_ref(), expr, params.len());
        }
    }
}
