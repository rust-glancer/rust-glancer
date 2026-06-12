//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_ir_model::{BindingId, BodyRef, ExprId};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{ExpectedNominalTyExt, PrimitiveTy, Ty};

use crate::{
    ir::body::ResolvedBodyData,
    ir::resolved::BodyResolution,
    ir::{BindingKind, BodySelfParamKind, ExprWrapperKind},
};

use crate::resolution::{
    BodyResolutionContext, BodyResolutionProviders, TypeRefUseSite, infer::BodyInferenceCtx,
};

use super::{
    expr::ExprResolutionPass, inference::InferenceResolutionPass,
    pattern_binding::PatternBindingMaterializationPass, pattern_type::PatternTypePropagationPass,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// Sibling pass modules keep their logic in separate files while operating on the same body
/// facts, so the fields are scoped to `resolution` rather than hidden inside this file.
pub(crate) struct BodyResolutionPass<'query, 'body, D, I> {
    pub(super) providers: BodyResolutionProviders<'query, D, I>,
    pub(super) body: &'body mut ResolvedBodyData,
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
        body: &'body mut ResolvedBodyData,
    ) -> Result<Self, PackageStoreError> {
        let providers =
            BodyResolutionProviders::new(def_maps, item_stores, semantic_index, body_ref);

        // Pattern materialization rewrites pending binding ids into the final binding arena.
        // Every later resolution step, including inference storage, assumes that stable shape.
        PatternBindingMaterializationPass::new(providers, body).materialize()?;
        let inference = BodyInferenceCtx::new(body.exprs().len(), body.bindings().len());

        Ok(Self {
            providers,
            body,
            inference,
        })
    }

    pub(super) fn context<'source>(
        &'source self,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        self.providers.context(self.body)
    }

    pub(crate) fn resolve(&mut self) -> Result<(), PackageStoreError> {
        self.resolve_bindings()?;

        // Pattern propagation can unlock later expression types, and those expressions can then
        // unlock more patterns. Every successful pass should discover at least one new binding or
        // expression fact, so a body-sized cap is enough to avoid a hidden magic constant.
        let max_passes = self.body.exprs().len() + self.body.bindings().len() + 1;
        for _ in 0..max_passes {
            let mut changed = false;
            let expr_count = self.body.exprs().len();
            {
                let mut expr_pass = ExprResolutionPass::new(self);
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

        InferenceResolutionPass::new(self).run()?;
        Ok(())
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
            if !matches!(self.body.binding_ty_unchecked(binding), Ty::Unknown) {
                continue;
            }

            self.set_binding_ty(binding, ty);
            changed = true;
        }

        changed
    }

    pub(super) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
        self.body.set_expr_ty(expr, ty);
    }

    pub(super) fn set_expr_integer_var(&mut self, expr: ExprId) {
        self.inference.set_expr_integer_var(expr);
        self.body
            .set_expr_ty(expr, Ty::Primitive(PrimitiveTy::DEFAULT_INT));
    }

    pub(super) fn set_expr_float_var(&mut self, expr: ExprId) {
        self.inference.set_expr_float_var(expr);
        self.body
            .set_expr_ty(expr, Ty::Primitive(PrimitiveTy::DEFAULT_FLOAT));
    }

    pub(super) fn set_expr_tuple_from_fields(&mut self, expr: ExprId, fields: &[ExprId]) {
        self.inference.set_expr_tuple_from_fields(expr, fields);
        self.body.set_expr_ty(
            expr,
            Ty::tuple(
                fields
                    .iter()
                    .map(|field| self.body.expr_ty_unchecked(*field).clone())
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
        self.body.set_expr_ty(expr, ty);
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
        self.body.set_expr_ty(expr, ty);
    }

    pub(super) fn set_expr_facts(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
        self.body.set_expr_facts(expr, resolution, ty);
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
        self.body.set_expr_facts(expr, resolution, ty);
    }

    fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) {
        self.inference.set_binding_ty(binding, &ty);
        self.body.set_binding_ty(binding, ty);
    }

    fn binding_ty(&self, binding: BindingId) -> Result<Ty, PackageStoreError> {
        let binding_data = self.body.binding_unchecked(binding);
        if let Some(annotation) = &binding_data.annotation {
            return self
                .context()
                .type_refs(TypeRefUseSite::Scope(binding_data.scope))
                .resolve(annotation);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
            && let Some(function) = self.body.function_owner()
        {
            let ty = self
                .context()
                .functions()
                .self_nominal_ty(function)?
                .into_self_ty();
            return Ok(match kind {
                BodySelfParamKind::Value => ty,
                BodySelfParamKind::Reference { mutability } => Ty::reference(mutability, ty),
                BodySelfParamKind::Explicit => Ty::Unknown,
            });
        }

        Ok(Ty::Unknown)
    }
}
