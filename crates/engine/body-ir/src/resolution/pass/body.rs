//! Main body-resolution pass.
//!
//! This module walks immutable body structure and derives resolution/type facts for bindings and
//! expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_def_map::DefMapSource;
use rg_ir_model::{BindingId, BodyRef, ExprId};
use rg_item_tree::SelfParamKind;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_ty::{ExpectedAdtTyExt, TraitSelectionSession, Ty};

use crate::{
    BodyData, BodyFacts,
    ir::resolved::BodyResolution,
    ir::{BindingKind, ExprWrapperKind},
};

use crate::resolution::{
    BodyResolutionContext,
    infer::{BodyInferenceCtx, BodyInferenceSnapshot},
};

use super::{
    env::BodyResolutionEnv, expr::ExprResolutionPass, inference::InferenceTransferPass,
    pattern::PatternInferenceTransfer,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// The structural body is borrowed and never mutated. Name resolution accumulates directly in the
/// pass-owned `BodyFacts`, while types and selected-call substitutions remain live in
/// `BodyInferenceCtx` until the fixed point settles. Sibling pass modules borrow this one state so
/// there is no second resolution lane to reconcile afterward.
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
        item_lookup_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        body: &'body BodyData,
        trait_selection: &'query TraitSelectionSession,
    ) -> Self {
        let env = BodyResolutionEnv::new(
            def_maps,
            item_stores,
            item_lookup_index,
            body_ref,
            trait_selection.for_body(body_ref),
        );

        let inference = BodyInferenceCtx::new(
            body.exprs().len(),
            body.bindings().len(),
            body.statements().len(),
        );

        Self {
            env,
            body,
            facts: BodyFacts::for_body(body),
            inference,
        }
    }

    pub(super) fn context<'source>(
        &'source self,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        self.env
            .context(self.inference.view(self.body, &self.facts))
    }

    /// Run a mutating inference operation against this transfer step's read snapshot.
    ///
    /// The context cannot borrow the live type slots while the operation mutates them. One
    /// copy-on-write snapshot is shared by every operation in the transfer step, so the live facts
    /// detach at most once before the outer fixed point creates the next read view.
    pub(super) fn with_context_and_inference<R>(
        &mut self,
        snapshot: &BodyInferenceSnapshot,
        operation: impl FnOnce(BodyResolutionContext<'_, &D, &I>, &mut BodyInferenceCtx) -> R,
    ) -> R {
        let Self {
            env,
            body,
            facts,
            inference,
        } = self;
        operation(env.context(snapshot.view(body, facts)), inference)
    }

    /// Resolve one frozen body and return its aligned, finalized semantic sidecar.
    ///
    /// Syntax-directed resolution first seeds what can be known immediately. The shared fixed
    /// point then lets expressions, patterns, calls, annotations, and trait obligations exchange
    /// evidence. Only after convergence are inference variables erased or defaulted into
    /// persistent `BodyFacts`.
    pub(crate) fn resolve(mut self) -> Result<BodyFacts, PackageStoreError> {
        self.resolve_bindings()?;

        // Seed syntax-directed expression facts before annotations introduce inference holes.
        // Calls and patterns that need later evidence are retried by the shared fixed point below.
        self.transfer_expressions_and_patterns()?;
        InferenceTransferPass::new(&mut self).initialize()?;

        // Expressions, patterns, calls, and expected types all exchange evidence through the same
        // inference context. Revisit the transfer rules while that context or declaration
        // resolution changes; there is no ordinary-fact pass to refresh afterward.
        let max_passes = self.body.exprs().len() + self.body.bindings().len() + 1;
        for _ in 0..max_passes {
            let before = self.inference.progress();
            let resolution_changed = self.transfer_expressions_and_patterns()?;
            InferenceTransferPass::new(&mut self).apply_once()?;

            if !resolution_changed && !self.inference.has_progressed_since(&before) {
                break;
            }
        }

        Ok(self.inference.finish(self.facts))
    }

    fn transfer_expressions_and_patterns(&mut self) -> Result<bool, PackageStoreError> {
        let mut resolution_changed = false;
        let expr_count = self.body.exprs().len();
        {
            let mut expr_pass = ExprResolutionPass::new(self);
            for expr_idx in 0..expr_count {
                resolution_changed |= expr_pass.resolve_expr(ExprId(expr_idx))?;
            }
        }
        let snapshot = self.inference.snapshot();
        self.with_context_and_inference(&snapshot, |context, inference| {
            PatternInferenceTransfer::new(context).propagate(inference)
        })?;
        Ok(resolution_changed)
    }

    fn resolve_bindings(&mut self) -> Result<(), PackageStoreError> {
        for binding_idx in 0..self.body.bindings().len() {
            let binding = BindingId(binding_idx);
            let ty = self.binding_ty(binding)?;
            self.set_binding_ty(binding, ty);
        }
        Ok(())
    }

    pub(super) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
    }

    pub(super) fn set_expr_facts(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty) {
        self.inference.set_expr_ty(expr, &ty);
        self.facts.set_expr_resolution(expr, resolution);
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
        self.facts.set_expr_resolution(expr, resolution);
    }

    pub(super) fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) {
        self.inference.set_binding_ty(binding, &ty);
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

    pub(super) fn expr_ty_unchecked(&self, expr: ExprId) -> &Ty {
        self.inference.expr_ty_ref(expr)
    }

    pub(super) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.facts.exprs[expr].resolution
    }

    pub(super) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.facts.set_expr_resolution(expr, resolution);
    }
}
