//! Instantiate each selected call once in the body's shared inference context.
//!
//! Arguments, expected returns, callable bounds, and projection results all constrain the same
//! variables. The signature is needed while checking arguments. Afterward, only the selected
//! function and its live generic arguments stay with the body; fulfillment owns outstanding goals.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, FunctionRef, GenericDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::solver::{CallableSignature, GenericArgs, InferenceTable, SolverInterner, Ty, TyShape};

use super::{BodyInference, deferred::DeferredKind};
use crate::{CallFacts, body::ExprKind};

/// What remains of a call after its arguments have been connected to the signature.
///
/// In `let value = make(); require_u8(value);`, `make<T>() -> T` is selected before `T` is known.
/// Retaining its live `?T` lets the later argument constrain the published call facts too.
/// The arguments include parent generics and keep declaration order for finalization.
pub(super) struct SelectedCall<'s> {
    function: FunctionRef,
    generic_args: GenericArgs<'s>,
}

impl<'s> SelectedCall<'s> {
    pub(super) fn function(&self) -> FunctionRef {
        self.function
    }

    pub(super) fn finalize(&self, table: &InferenceTable<'s>) -> CallFacts {
        CallFacts::new(self.function, table.finalize_args(self.generic_args))
    }
}

/// A call held while its arguments are being visited and connected to the signature.
/// The signature supplies argument expectations. Once checking is done, it and the receiver
/// information can be dropped; only `selected` is retained until body finalization.
pub(super) struct PreparedCall<'s> {
    call: ExprId,
    selected: SelectedCall<'s>,
    signature: CallableSignature<'s>,
    // `value.method(arg)` supplies `self` separately; `Type::method(value, arg)` writes it out.
    first_written_param_idx: usize,
    receiver_ty: Option<Ty<'s>>,
}

impl<'s> PreparedCall<'s> {
    fn argument_expected_ty(
        &self,
        index: usize,
        arg_count: usize,
        cx: SolverInterner<'s>,
    ) -> Ty<'s> {
        let params = &self.signature.params[self.first_written_param_idx..];
        if params.len() == arg_count {
            params[index]
        } else {
            cx.unknown()
        }
    }
}

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Calls establish closure signatures early, then process ordinary arguments and fulfill
    /// their bounds before consuming closure bodies. Those bodies are still visited exactly once.
    /// For example, in `apply(|user| user.name, user)`, the second argument and a callable bound
    /// can tell us the closure parameter's type before we attempt the field lookup in its body.
    pub(super) fn infer_call(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
        expected: &Ty<'s>,
    ) -> anyhow::Result<()> {
        for arg in args {
            self.prepare_closure(*arg);
        }
        let lookup_input = receiver.map(|expr| {
            self.inference
                .table()
                .canonicalize(&self.inference.expr_ty(expr))
        });
        let prepared = self
            .prepare_call(call, receiver)
            .context("select call signature")?;
        self.inference.expr_slot(call);
        // A selected generic signature can use the expectation to infer its type arguments.
        // An unresolved call or root projection may instead turn out to return `!`; its expected
        // type is applied by the expression's coercion after the pending work is registered.
        if prepared.is_some()
            && !self
                .inference
                .table()
                .has_pending_projection(self.inference.root_resolved_expr_ty(call))
        {
            self.coerce_expr_ty(call, expected);
        }
        for (index, arg) in args.iter().enumerate() {
            if matches!(
                self.body.expr_unchecked(*arg).kind,
                ExprKind::Closure { .. }
            ) {
                continue;
            }
            let expected = prepared
                .as_ref()
                .map(|prepared| prepared.argument_expected_ty(index, args.len(), self.cx))
                .unwrap_or(self.cx.unknown());
            self.infer_expr(*arg, &expected)
                .context("infer call argument")?;
        }
        if let Some(prepared) = prepared {
            self.finish_call(prepared, args);
        } else {
            // Only lookup needs a body-level retry. Once a call is selected, its remaining
            // predicates and projections are already in the inference table's goal queue.
            let pending = DeferredKind::Call { call };
            let receiver_changed = lookup_input
                != receiver.map(|expr| {
                    self.inference
                        .table()
                        .canonicalize(&self.inference.expr_ty(expr))
                });
            if receiver_changed {
                // Ordinary argument inference can refine the same local used as this receiver.
                self.run_or_defer(pending)
                    .context("retry call after argument inference")?;
            } else {
                self.defer(pending, None);
            }
        }

        // Callable bounds can now constrain the prepared closure signatures. Make that evidence
        // available before the closures introduce their parameter bindings and walk their bodies.
        self.fulfill_pending()
            .context("fulfill argument obligations")?;
        for arg in args {
            if matches!(
                self.body.expr_unchecked(*arg).kind,
                ExprKind::Closure { .. }
            ) {
                self.infer_expr(*arg, &self.cx.unknown())
                    .context("infer closure argument")?;
            }
        }

        // Closure return types can unlock more obligations, such as the item type of a mapped
        // iterator. Give pending operations a chance to use that evidence before leaving the call.
        self.fulfill_pending()
            .context("fulfill closure obligations")?;
        Ok(())
    }

    /// Pick a unique usable target and instantiate its signature before visiting arguments.
    /// The target's trial table carries any receiver constraints found during lookup. Adopting it
    /// keeps those constraints and its pending bounds connected to the call's new variables.
    pub(super) fn prepare_call(
        &mut self,
        call: ExprId,
        receiver: Option<ExprId>,
    ) -> Result<Option<PreparedCall<'s>>, PackageStoreError> {
        crate::profile::metric::CALL_ATTEMPTS.inc();
        let resolution = match self.body.expr_unchecked(call).kind {
            ExprKind::Call {
                callee: Some(callee),
                ..
            } => Some(self.inference.expr_resolution(callee)),
            _ => None,
        };
        let receiver = receiver.map(|receiver| self.inference.root_resolved_expr_ty(receiver));
        let mut targets = self
            .context
            .live()
            .call_targets(call, resolution, receiver, self.inference.table())?
            .into_iter()
            .filter(|t| t.can_infer);
        let Some(target) = targets.next() else {
            return Ok(None);
        };
        if targets.next().is_some() {
            return Ok(None);
        }
        self.inference.table.adopt(target.table);
        let function = target.function;
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Function(function))?;
        let mut subst = target.subst;
        if !target.explicit_args.is_empty() {
            let args = self.context.live().generic_args(
                target.scope,
                &generics,
                &target.explicit_args,
                self.inference.table(),
            )?;
            for (param, arg) in generics
                .iter_self()
                .zip(args.iter().skip(generics.parent_len()))
            {
                subst.insert(param.param(), arg);
            }
        }
        subst.fresh_for(self.inference.table(), generics.iter().map(|p| p.param()));
        let Some(signature) = self
            .inference
            .table()
            .instantiate_function(function, &subst)
        else {
            return Ok(None);
        };
        // Normalize once and retain the resulting slots. The solver retries the generated goals
        // when arguments or expectations make progress, without rebuilding the call substitution.
        let return_ty = self.inference.table().normalize(signature.ret);
        self.inference.set_expr_ty(call, return_ty);
        Ok(Some(PreparedCall {
            call,
            selected: SelectedCall {
                function,
                generic_args: subst.args_for(self.cx, generics.iter().map(|p| p.param())),
            },
            signature,
            first_written_param_idx: target.first_written,
            receiver_ty: target.receiver,
        }))
    }

    /// Connect argument results to the prepared signature, then retain the call for finalization.
    /// Bounds can remain pending: later body evidence still reaches their shared variables.
    pub(super) fn finish_call(&mut self, prepared: PreparedCall<'s>, args: &[ExprId]) {
        for (arg, param) in args.iter().zip(
            prepared
                .signature
                .params
                .iter()
                .skip(prepared.first_written_param_idx),
        ) {
            self.coerce_expr_ty(*arg, &param);
        }
        if prepared.first_written_param_idx == 1
            && let Some(receiver) = prepared.receiver_ty
            && let Some(param) = prepared.signature.params.first()
        {
            let param = match param.shape() {
                TyShape::Reference { inner, .. } => inner,
                _ => *param,
            };
            self.inference.table().unify(receiver, param);
        }
        let _ = self.inference.table().fulfill();
        self.inference
            .set_selected_call(prepared.call, prepared.selected);
    }
}
