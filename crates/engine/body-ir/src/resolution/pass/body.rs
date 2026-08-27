//! Main body-resolution pass.
//!
//! This module walks immutable body structure and derives resolution/type facts for bindings and
//! expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_def_map::DefMapSource;
use rg_ir_model::{BindingId, BodyRef, ExprId};
use rg_item_tree::SelfParamKind;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupQuery, ItemStoreSource};
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

// Most bodies settle after a handful of rounds, but a single slot may legitimately gain several
// layers of evidence. Keep the emergency ceiling independent of body size: slot count is not a
// sound convergence bound, while a fixed high limit still prevents an endless rescan.
const MAX_BODY_INFERENCE_PASSES: usize = 128;

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
        item_lookup_query: &'query ItemLookupQuery<'query>,
        body_ref: BodyRef,
        body: &'body BodyData,
        trait_selection: &'query TraitSelectionSession,
    ) -> Self {
        let env = BodyResolutionEnv::new(
            def_maps,
            item_stores,
            item_lookup_query,
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

        let has_method_calls = self
            .body
            .exprs()
            .iter()
            .any(|expr| matches!(&expr.kind, crate::ir::ExprKind::MethodCall { .. }));

        // Seed syntax-directed expression facts before annotations introduce inference holes.
        // Method declarations are deferred until call inference has had a chance to retain a
        // unique target: resolving them here would perform the same impl search once for the
        // expression fact and again for the call signature.
        self.transfer_expressions_and_patterns()?;
        InferenceTransferPass::new(&mut self).initialize()?;

        // Reapply syntax after annotations have reached their expressions, then let calls retain
        // their unique target before the ordinary fixed point asks for declaration facts. For
        // `value.convert()`, the following expression pass can read that target directly. Calls
        // that remain ambiguous have no retained state and still use full editor-facing lookup.
        // Bodies without method syntax keep the ordinary path and do not pay for an extra seed.
        if has_method_calls {
            self.transfer_expressions_and_patterns()?;
            InferenceTransferPass::new(&mut self).apply_once()?;
        }

        // Expressions, patterns, calls, and expected types all exchange evidence through the same
        // inference context. Method declarations are editor-facing facts rather than inference
        // inputs: call inference performs the lookup that can retain a unique target. Defer the
        // declaration-only lookup so an unresolved `value.method()` is not expanded once here and
        // once in call inference on every fixed-point round.
        let mut converged = false;
        let mut final_resolution_changed = false;
        let mut final_inference_changed = false;
        for _ in 0..MAX_BODY_INFERENCE_PASSES {
            let before = self.inference.progress();
            let resolution_changed = self.transfer_expressions_and_patterns()?;
            InferenceTransferPass::new(&mut self).apply_once()?;
            let inference_changed = self.inference.has_progressed_since(&before);

            if !resolution_changed && !inference_changed {
                converged = true;
                break;
            }
            final_resolution_changed = resolution_changed;
            final_inference_changed = inference_changed;
        }

        if !converged {
            crate::profile::metric::FIXED_POINT_EXHAUSTIONS.inc();
            tracing::warn!(
                body = ?self.env.body_ref(),
                owner = ?self.body.owner(),
                source = ?self.body.source(),
                max_passes = MAX_BODY_INFERENCE_PASSES,
                expression_count = self.body.exprs().len(),
                binding_count = self.body.bindings().len(),
                final_resolution_changed,
                final_inference_changed,
                "body inference stopped at the fixed-point pass limit; unresolved facts remain unknown"
            );
        }

        // Selected calls already retain their declaration, while ambiguous calls still need the
        // broader editor-facing result. Resolve both once from the strongest receiver types the
        // fixed point produced. This fact cannot make another inference rule applicable, so it
        // deliberately sits outside the convergence loop.
        if has_method_calls {
            ExprResolutionPass::new(&mut self).resolve_method_declarations()?;
        }

        Ok(self.inference.finish(self.facts, converged))
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
