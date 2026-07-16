//! Sources of expected types for inference-aware pattern projection.
//!
//! This transfer walks the body places that introduce patterns. The recursive pattern semantics
//! themselves live in `BodyPatternInference`, so a function parameter, a `let`, a match arm, and a
//! callable obligation all project tuple/record/variant fields through the same implementation.

mod callable;

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ScopeId, StmtId, items::TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::Ty;

use crate::ir::{ExprKind, StmtKind};
use crate::resolution::{
    BodyResolutionContext,
    infer::{BodyInferenceCtx, BodyPatternInference},
};

use self::callable::CallableInputExpectation;

/// Routes each body-level source of an expected type into recursive pattern inference.
///
/// This pass finds the surrounding contract: a function parameter, `let` initializer, match
/// scrutinee, iterator item, closure annotation, or callable argument. `BodyPatternInference`
/// owns the actual tuple, record, variant, and binding projection so every source follows the same
/// pattern semantics.
pub(super) struct PatternInferenceTransfer<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> PatternInferenceTransfer<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Push every available body expectation through its pattern once.
    pub(super) fn propagate(
        &self,
        inference: &mut BodyInferenceCtx,
    ) -> Result<(), PackageStoreError> {
        let patterns = BodyPatternInference::new(self.context);

        // Function parameters retain their root patterns even though body consumers see flattened
        // bindings. Canonical signatures keep APIT, inherited generics, and `Self` identities.
        let signature = self
            .context
            .body()
            .owner()
            .function()
            .map(|function| self.context.signatures().function(function))
            .transpose()?
            .flatten();
        for (param_index, param) in self.context.body().function_params().iter().enumerate() {
            let Some(pat) = param.pat else {
                continue;
            };
            let expected_ty = match signature
                .as_ref()
                .and_then(|signature| signature.params.get(param_index))
            {
                Some(ty) => ty.clone(),
                None => {
                    let Some(annotation) = param.annotation.as_ref() else {
                        continue;
                    };
                    self.context
                        .type_refs(self.context.body().param_scope())
                        .resolve(annotation)?
                }
            };
            patterns.link_pat(inference, pat, &expected_ty)?;
        }

        for statement_idx in 0..self.context.body().statements().len() {
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                annotation,
                initializer,
                ..
            } = self
                .context
                .body()
                .statement_unchecked(StmtId(statement_idx))
                .kind
                .clone()
            else {
                continue;
            };

            let expected_ty =
                self.expected_ty_for_let(inference, scope, annotation.as_ref(), initializer)?;
            patterns.link_pat(inference, pat, &expected_ty)?;
        }

        let iteration_items = self.context.iteration_items();
        for expr_idx in 0..self.context.body().exprs().len() {
            let expr = ExprId(expr_idx);
            match self.context.body().expr_unchecked(expr).kind.clone() {
                ExprKind::Match { scrutinee, arms } => {
                    let Some(scrutinee) = scrutinee else {
                        continue;
                    };
                    let expected_ty = inference.root_resolved_expr_ty(scrutinee);
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            patterns.link_pat(inference, pat, &expected_ty)?;
                        }
                    }
                }
                ExprKind::Let {
                    scope,
                    pat: Some(pat),
                    initializer,
                    ..
                } => {
                    let expected_ty =
                        self.expected_ty_for_let(inference, scope, None, initializer)?;
                    patterns.link_pat(inference, pat, &expected_ty)?;
                }
                ExprKind::For {
                    pat: Some(pat),
                    iterable: Some(iterable),
                    ..
                } => {
                    let iterable_ty = inference.root_resolved_expr_ty(iterable);
                    let item_ty = iteration_items.into_iterator_item_for_ty(&iterable_ty)?;
                    patterns.link_pat(inference, pat, &item_ty)?;
                }
                ExprKind::Closure { scope, params, .. } => {
                    for param in params {
                        let (Some(pat), Some(annotation)) = (param.pat, param.annotation) else {
                            continue;
                        };
                        let expected_ty = self.context.type_refs(scope).resolve(&annotation)?;
                        patterns.link_pat(inference, pat, &expected_ty)?;
                    }
                }
                ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                    self.propagate_closure_arg_expectations(inference, &patterns, expr, &args)?;
                }
                ExprKind::Path { .. }
                | ExprKind::Tuple { .. }
                | ExprKind::Array { .. }
                | ExprKind::RepeatArray { .. }
                | ExprKind::Index { .. }
                | ExprKind::Range { .. }
                | ExprKind::Cast { .. }
                | ExprKind::Unary { .. }
                | ExprKind::Binary { .. }
                | ExprKind::Assign { .. }
                | ExprKind::If { .. }
                | ExprKind::Loop { .. }
                | ExprKind::While { .. }
                | ExprKind::For { .. }
                | ExprKind::Break { .. }
                | ExprKind::Continue { .. }
                | ExprKind::Block { .. }
                | ExprKind::Field { .. }
                | ExprKind::Record { .. }
                | ExprKind::Wrapper { .. }
                | ExprKind::BuiltinMacro { .. }
                | ExprKind::Literal { .. }
                | ExprKind::Underscore
                | ExprKind::Yield { .. }
                | ExprKind::Yeet { .. }
                | ExprKind::Become { .. }
                | ExprKind::Let { pat: None, .. }
                | ExprKind::Unknown { .. } => {}
            }
        }

        Ok(())
    }

    /// Propagate callable input contracts only into closure argument patterns.
    fn propagate_closure_arg_expectations(
        &self,
        inference: &mut BodyInferenceCtx,
        patterns: &BodyPatternInference<'query, D, I>,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let receiver_ty = match self.context.body().expr_unchecked(call).kind {
            ExprKind::MethodCall {
                receiver: Some(receiver),
                ..
            } => Some(inference.root_resolved_expr_ty(receiver)),
            _ => None,
        };
        for (arg, expectation) in
            CallableInputExpectation::for_call(self.context, call, args, receiver_ty.as_ref())?
        {
            let ExprKind::Closure { params, .. } =
                self.context.body().expr_unchecked(arg).kind.clone()
            else {
                continue;
            };
            if params.len() != expectation.params.len() {
                continue;
            }

            for (param, expected_ty) in params.iter().zip(&expectation.params) {
                let Some(pat) = param.pat else {
                    continue;
                };
                patterns.link_pat(inference, pat, expected_ty)?;
            }
        }
        Ok(())
    }

    fn expected_ty_for_let(
        &self,
        inference: &BodyInferenceCtx,
        scope: ScopeId,
        annotation: Option<&TypeRef>,
        initializer: Option<ExprId>,
    ) -> Result<Ty, PackageStoreError> {
        if let Some(annotation) = annotation {
            let ty = self.context.type_refs(scope).resolve(annotation)?;
            if !matches!(ty, Ty::Unknown) {
                return Ok(ty);
            }
        }

        Ok(initializer
            .map(|expr| inference.root_resolved_expr_ty(expr))
            .unwrap_or(Ty::Unknown))
    }
}
