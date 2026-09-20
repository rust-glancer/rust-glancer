//! Recursive body inference and the temporary state used to produce durable body facts.
//!
//! The context starts with parameter types, then walks expressions and patterns from the body root.
//! Each expression records its type and connects it to its children through shared inference
//! variables. Later evidence can travel through those variables without walking the syntax again.
//!
//! `unify` keeps these relationships, `call` retains each selected function's substitutions, and
//! `fulfill` retries lookups and projections that need more evidence. Completion resolves the live
//! variables before moving the expression and binding facts into `BodyFacts`.

use std::collections::VecDeque;

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{BodyRef, ExprId, identity::DeclarationRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupQuery, ItemStoreSource};
use rg_std::OperationError;
use rg_ty::Ty;
use rg_ty::trait_selection::TraitSelectionSession;

use crate::{BodyData, BodyFacts, body::ExprKind, body::facts::BodyResolution};

use crate::resolution::BodyResolutionContext;

mod builtin_macro;
mod call;
mod closure;
mod expr;
mod fulfill;
mod pat;
mod unify;

use fulfill::Deferred;
use unify::InferenceState;

/// One body's recursive inference operation. Structure and semantic query inputs are immutable;
/// live types, selected calls, and pending work are owned until the final sidecar is published.
pub(crate) struct InferenceContext<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    body: &'query BodyData,
    inference: InferenceState,
    deferred: VecDeque<Deferred>,
    // Navigation candidates are collected after receiver types have had the whole body to settle.
    method_calls: Vec<ExprId>,
    return_ty: Ty,
    inference_exhausted: bool,
    depth: usize,
}

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(
        def_maps: D,
        item_stores: I,
        item_lookup_query: &ItemLookupQuery<'query>,
        body_ref: BodyRef,
        body: &'query BodyData,
        trait_selection: &TraitSelectionSession,
    ) -> Self {
        let context = BodyResolutionContext::new(
            def_maps,
            item_stores,
            body_ref,
            body,
            item_lookup_query,
            trait_selection.for_body(body_ref),
        );

        let inference = InferenceState::new(body.exprs().len(), body.bindings().len());

        Self {
            context,
            body,
            inference,
            deferred: VecDeque::new(),
            method_calls: Vec::new(),
            return_ty: Ty::Unknown,
            inference_exhausted: false,
            depth: 0,
        }
    }

    /// Infer the body once, finish pending semantic work, then publish durable facts.
    #[rg_std::cancelable("start body inference", token = self.context)]
    pub(crate) fn infer_body(mut self) -> anyhow::Result<BodyFacts> {
        // Make declaration types available before visiting any parameter uses or return values.
        self.infer_parameters()
            .context("infer function parameters")?;
        self.infer_expr(self.body.root_expr(), &self.return_ty.clone())
            .context("infer root expression")?;
        // The last subtree may have supplied evidence for earlier operations. Complete those
        // before choosing the declarations that editor queries will see.
        self.fulfill_pending().context("complete body inference")?;
        self.resolve_method_declarations()
            .context("resolve method declarations")?;

        if self.inference_exhausted {
            tracing::warn!(body = ?self.context.body_ref(), "body inference stopped at a work limit; publishing partial results");
        }

        // A cancelled solver may return a conservative answer. Do not publish that answer as a
        // completed body.
        rg_std::check_cancel!(self.context, "finalize body facts");
        Ok(self.finish_coercions().finish())
    }

    /// Populate editor-facing declarations after inference settles. A uniquely selected call
    /// keeps its target; otherwise navigation can retain the broader lookup candidates.
    fn resolve_method_declarations(&mut self) -> Result<(), OperationError<PackageStoreError>> {
        for call in std::mem::take(&mut self.method_calls) {
            rg_std::check_cancel!(self.context, "method declaration resolution");
            let ExprKind::MethodCall { receiver, .. } = self.body.expr_unchecked(call).kind else {
                continue;
            };

            // Repeating lookup for a selected call could disagree with the target whose
            // signature and substitutions already determined the inferred types.
            let resolution = if let Some(function) = self.inference.selected_call_function(call) {
                BodyResolution::Declarations([DeclarationRef::from(function)].into_iter().collect())
            } else if let Some(receiver) = receiver {
                let receiver_ty = self.inference.root_resolved_expr_ty(receiver);
                let targets = self
                    .context
                    .calls()
                    .method_targets(call, &receiver_ty, self.inference.table())
                    .map_err(OperationError::Source)?;
                if targets.is_empty() {
                    BodyResolution::Unknown
                } else {
                    targets.resolution()
                }
            } else {
                BodyResolution::Unknown
            };
            self.inference.set_expr_resolution(call, resolution);
        }
        Ok(())
    }

    #[cfg(test)]
    fn before_expression(&self) {
        tests::before_expression(self.context.ty_context().trait_selection().cancellation());
    }
}

#[cfg(test)]
mod tests;
