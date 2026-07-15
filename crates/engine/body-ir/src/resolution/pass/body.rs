//! Main body-resolution pass.
//!
//! This module walks immutable body structure and derives resolution/type facts for bindings and
//! expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_def_map::DefMapSource;
use rg_ir_model::{BindingId, BodyRef, ExprId, items::SelfParamKind};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_ty::{ExpectedAdtTyExt, PrimitiveTy, TraitSelectionCache, Ty};

use crate::{
    BodyData, BodyFacts, BodyView,
    ir::resolved::BodyResolution,
    ir::{BindingKind, ExprWrapperKind},
};

use crate::resolution::{BodyResolutionContext, infer::BodyInferenceCtx};

use super::{
    env::BodyResolutionEnv, expr::ExprResolutionPass, inference::InferenceResolutionPass,
    pattern_type::PatternTypePropagationPass,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// Sibling pass modules keep their logic in separate files while operating on the same fact
/// sidecar. The structural body is borrowed for the entire pass and never mutated.
pub(crate) struct BodyResolutionPass<'query, 'body, D, I> {
    pub(super) env: BodyResolutionEnv<'query, D, I>,
    pub(super) body: &'body BodyData,
    pub(super) facts: BodyFacts,
    pub(super) inference: BodyInferenceCtx,
}

impl<'query, 'body, D, I> BodyResolutionPass<'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(crate) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        body: &'body BodyData,
        facts: BodyFacts,
        trait_selection_cache: &'query TraitSelectionCache,
    ) -> Self {
        debug_assert!(facts.is_aligned_with(body));
        let env = BodyResolutionEnv::new(
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
            trait_selection_cache,
        );

        let inference = BodyInferenceCtx::with_trait_selection_cache(
            body.exprs().len(),
            body.bindings().len(),
            trait_selection_cache.clone(),
        );

        Self {
            env,
            body,
            facts,
            inference,
        }
    }

    pub(super) fn context<'source>(
        &'source self,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        self.env.context(self.view())
    }

    /// Split read-only query state from the inference table that a query operation may update.
    ///
    /// Building the context from `self.context()` would borrow the whole pass and prevent the
    /// operation from mutating inference. Keeping this split here avoids exposing the pass's field
    /// layout throughout inference code.
    pub(super) fn context_and_inference<'source>(
        &'source mut self,
    ) -> (
        BodyResolutionContext<'source, &'source D, &'source I>,
        &'source mut BodyInferenceCtx,
    ) {
        let Self {
            env,
            body,
            facts,
            inference,
        } = self;
        (env.context(BodyView::new(body, facts)), inference)
    }

    pub(crate) fn resolve(mut self) -> Result<BodyFacts, PackageStoreError> {
        self.resolve_bindings()?;

        // Pattern propagation can unlock later expression types, and those expressions can then
        // unlock more patterns. Every successful pass should discover at least one new binding or
        // expression fact, so a body-sized cap is enough to avoid a hidden magic constant.
        let max_passes = self.body.exprs().len() + self.body.bindings().len() + 1;
        for _ in 0..max_passes {
            let mut changed = false;
            let expr_count = self.body.exprs().len();
            {
                let mut expr_pass = ExprResolutionPass::new(&mut self);
                for expr_idx in 0..expr_count {
                    changed |= expr_pass.resolve_expr(ExprId(expr_idx))?;
                }
            }
            let binding_updates = PatternTypePropagationPass::new(self.context()).propagate()?;
            changed |= self.apply_binding_type_updates(binding_updates);

            if !changed {
                break;
            }
        }

        InferenceResolutionPass::new(&mut self).run()?;
        Ok(self.facts)
    }

    fn resolve_bindings(&mut self) -> Result<(), PackageStoreError> {
        for binding_idx in 0..self.body.bindings().len() {
            let binding = BindingId(binding_idx);
            let ty = self.binding_ty(binding)?;
            self.set_binding_ty(binding, ty);
        }
        Ok(())
    }

    fn apply_binding_type_updates(&mut self, updates: Vec<(BindingId, Ty)>) -> bool {
        let mut changed = false;
        for (binding, ty) in updates {
            if matches!(ty, Ty::Unknown) {
                continue;
            }

            if self.body.binding(binding).is_none() {
                continue;
            };
            if !matches!(self.binding_ty_unchecked(binding), Ty::Unknown) {
                continue;
            }

            self.set_binding_ty(binding, ty);
            changed = true;
        }

        changed
    }

    pub(super) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
        self.facts.set_expr_ty(expr, ty);
    }

    pub(super) fn set_expr_integer_var(&mut self, expr: ExprId) {
        self.inference.set_expr_integer_var(expr);
        self.facts
            .set_expr_ty(expr, Ty::Primitive(PrimitiveTy::DEFAULT_INT));
    }

    pub(super) fn set_expr_float_var(&mut self, expr: ExprId) {
        self.inference.set_expr_float_var(expr);
        self.facts
            .set_expr_ty(expr, Ty::Primitive(PrimitiveTy::DEFAULT_FLOAT));
    }

    pub(super) fn set_expr_tuple_from_fields(&mut self, expr: ExprId, fields: &[ExprId]) {
        self.inference.set_expr_tuple_from_fields(expr, fields);
        self.facts.set_expr_ty(
            expr,
            Ty::tuple(
                fields
                    .iter()
                    .map(|field| self.expr_ty_unchecked(*field).clone())
                    .collect(),
            ),
        );
    }

    pub(super) fn set_expr_array_from_elements(
        &mut self,
        expr: ExprId,
        elements: &[ExprId],
        ty: Ty,
    ) {
        self.inference.set_expr_array_from_elements(
            expr,
            elements,
            Some(elements.len().to_string()),
        );
        self.facts.set_expr_ty(expr, ty);
    }

    pub(super) fn set_expr_repeat_array_from_initializer(
        &mut self,
        expr: ExprId,
        initializer: Option<ExprId>,
        len_text: Option<&str>,
        ty: Ty,
    ) {
        self.inference.set_expr_repeat_array_from_initializer(
            expr,
            initializer,
            len_text.map(str::to_owned),
        );
        self.facts.set_expr_ty(expr, ty);
    }

    pub(super) fn set_expr_facts(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
        self.facts.set_expr(expr, resolution, ty);
    }

    pub(super) fn set_expr_wrapper_facts(
        &mut self,
        expr: ExprId,
        resolution: BodyResolution,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
        ty: Ty,
    ) {
        self.inference
            .set_expr_wrapper_from_inner(expr, kind, inner, &ty);
        self.facts.set_expr(expr, resolution, ty);
    }

    pub(super) fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) {
        self.inference.set_binding_ty(binding, &ty);
        self.facts.set_binding_ty(binding, ty);
    }

    fn binding_ty(&self, binding: BindingId) -> Result<Ty, PackageStoreError> {
        let binding_data = self.body.binding_unchecked(binding);
        if matches!(
            binding_data.kind,
            BindingKind::Param | BindingKind::SelfParam(_)
        ) && let Some(function) = self.body.owner().function()
            && let Some(param_index) = self.body.function_param_index_for_binding(binding)
            && self.body.function_params()[param_index].bindings.len() == 1
            && let Some(signature) = self.context().signatures().function(function)?
            && let Some(param_ty) = signature.params.get(param_index)
            && !matches!(param_ty, Ty::Unknown)
        {
            return Ok(param_ty.clone());
        }

        if let Some(annotation) = &binding_data.annotation {
            return self
                .context()
                .type_refs(binding_data.scope)
                .resolve(annotation);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
            && let Some(function) = self.body.owner().function()
        {
            let ty = self
                .context()
                .functions()
                .self_adt_ty(function)?
                .into_adt_ty();
            return Ok(match kind {
                SelfParamKind::Value => ty,
                SelfParamKind::Reference { mutability } => Ty::reference(mutability, ty),
                SelfParamKind::Explicit => Ty::Unknown,
            });
        }

        Ok(Ty::Unknown)
    }

    pub(super) fn view(&self) -> BodyView<'_> {
        BodyView::new(self.body, &self.facts)
    }

    pub(super) fn expr_ty_unchecked(&self, expr: ExprId) -> &Ty {
        &self.facts.exprs[expr].ty
    }

    pub(super) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.facts.exprs[expr].resolution
    }

    pub(super) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.facts.set_expr_resolution(expr, resolution);
    }

    pub(super) fn binding_ty_unchecked(&self, binding: BindingId) -> &Ty {
        &self.facts.bindings[binding].ty
    }
}
