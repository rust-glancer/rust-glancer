//! Sources of expected types for inference-aware pattern projection.
//!
//! This transfer walks the body places that introduce patterns. The recursive pattern semantics
//! themselves live in `BodyPatternInference`, so function parameters, `let` bindings, match arms,
//! iterator items, and closure signatures all use the same tuple/record/variant implementation.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ItemOwner, ScopeId, StmtId, TraitDefRef};
use rg_item_tree::{LangItem, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{GenericArgs, TraitGoal, Ty};

use crate::ir::{ExprKind, StmtKind};
use crate::resolution::{
    BodyResolutionContext,
    infer::{BodyInferenceCtx, BodyPatternInference},
};

/// Routes each body-level source of an expected type into recursive pattern inference.
///
/// This pass finds the surrounding contract: a function parameter, `let` initializer, match
/// scrutinee, iterator item, or closure signature. `BodyPatternInference`
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
        let patterns = BodyPatternInference::new(self.context.clone());

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
                    let item_ty = if matches!(iterable_ty, Ty::Unknown) {
                        Ty::Unknown
                    } else {
                        // `for pat in value` has no source expression for the implicit
                        // `IntoIterator::into_iter` call. Model its item type as the ordinary
                        // `<typeof(value) as IntoIterator>::Item` projection, using the same live
                        // inference table as the rest of the body.
                        let lang_items = self.context.item_lookup_query();
                        let Some(into_iter) = lang_items.lang_function(LangItem::IntoIter) else {
                            continue;
                        };
                        let Some(into_iter_data) =
                            self.context.item_query().function_data(into_iter)?
                        else {
                            continue;
                        };
                        let ItemOwner::Trait(into_iterator_id) = into_iter_data.owner else {
                            continue;
                        };
                        let goal = TraitGoal::new(
                            iterable_ty,
                            TraitDefRef::new(into_iter.origin, into_iterator_id),
                            GenericArgs::empty(),
                        );
                        let query = self.context.trait_selection();
                        let Some(projection) =
                            query.normalize_assoc_type(&goal, "Item", inference.table())?
                        else {
                            continue;
                        };
                        let item_ty = projection.ty;
                        *inference.table_mut() = projection.table;
                        inference.root_resolved_ty(&item_ty)
                    };
                    patterns.link_pat(inference, pat, &item_ty)?;
                }
                ExprKind::Closure {
                    scope,
                    params,
                    ret_ty,
                    body,
                    ..
                } => {
                    // Keep the closure-owned slots themselves, not their interim canonical
                    // values. A return slot may first learn `Option<unknown>` from the body and
                    // later become `Option<User>` through the variant payload. Canonicalizing here
                    // would replace that slot with the weak shape and sever the later evidence.
                    let Ty::Closure(signature) = inference.expr_ty(expr) else {
                        continue;
                    };
                    for (param, signature_ty) in params.iter().zip(&signature.params) {
                        if let Some(annotation) = &param.annotation {
                            let annotation_ty =
                                self.context.type_refs(scope).resolve(annotation)?;
                            inference.constrain_infer_tys(signature_ty, &annotation_ty);
                        }
                        if let Some(pat) = param.pat {
                            patterns.link_pat(inference, pat, signature_ty)?;
                        }
                    }
                    if let Some(ret_ty) = &ret_ty {
                        let annotation_ty = self.context.type_refs(scope).resolve(ret_ty)?;
                        inference.constrain_infer_tys(&signature.ret, &annotation_ty);
                    }
                    if let Some(body) = body {
                        // A body can reveal a useful outer shape before its children are known,
                        // as `(index, user)` does before a callable bound types either binding.
                        // Give those children stable slots before linking the body to the closure
                        // output; otherwise `(unknown, unknown)` would become terminal evidence
                        // that the trait solver cannot refine.
                        let body_ty = inference.expr_ty(body);
                        if body_ty.has_unknown() {
                            inference.instantiate_expr_nested_unknown_ty(body, &body_ty);
                        }
                        inference.constrain_expr_ty(body, &signature.ret);
                    }
                }
                ExprKind::Path { .. }
                | ExprKind::Tuple { .. }
                | ExprKind::Array { .. }
                | ExprKind::RepeatArray { .. }
                | ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
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
