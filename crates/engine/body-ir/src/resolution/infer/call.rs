//! Instantiate each selected call once in the body's shared inference context.
//!
//! Arguments, expected returns, callable bounds, and projection results all constrain the same
//! variables. The signature is needed while checking arguments. Afterward, only the selected
//! function and its live generic arguments stay with the body; fulfillment owns outstanding goals.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, GenericDefRef, ScopeId};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_ty::solver::{FnDefTy, List, SolverInterner, Ty, TyShape};

use super::{BodyInference, deferred::DeferredKind};
use crate::{body::ExprKind, resolution::query::LiveCallTarget};

/// A call held while its arguments are being visited and connected to the signature.
/// The signature supplies argument expectations. Once checking is done, it and the receiver
/// information can be dropped; only `selected` is retained until body finalization.
pub(super) struct PreparedCall<'s> {
    selected: Option<FnDefTy<'s>>,
    params: List<'s, Ty<'s>>,
    output: Ty<'s>,
    // A dot call supplies `self` separately; `Type::method(value, arg)` writes it out.
    receiver: Option<Ty<'s>>,
}

impl<'s> PreparedCall<'s> {
    fn from_item(
        item: FnDefTy<'s>,
        receiver: Option<Ty<'s>>,
        cx: SolverInterner<'s>,
    ) -> Option<Self> {
        let signature = item.signature(cx)?;
        Some(Self {
            selected: Some(item),
            params: signature.params,
            output: signature.call_return,
            receiver,
        })
    }

    fn argument_params(&self) -> &[Ty<'s>] {
        self.params
            .get(usize::from(self.receiver.is_some())..)
            .unwrap_or_default()
    }

    fn argument_expected_ty(
        &self,
        index: usize,
        arg_count: usize,
        cx: SolverInterner<'s>,
    ) -> Ty<'s> {
        let params = self.argument_params();
        if params.len() == arg_count {
            params[index]
        } else {
            cx.unknown()
        }
    }
}

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Calls establish closure signatures early, then process ordinary arguments and fulfill
    /// their bounds before consuming closure bodies. Those bodies are still visited exactly once.
    /// For example, in `apply(|user| user.name, user)`, the second argument and a callable bound
    /// can tell us the closure parameter's type before we attempt the field lookup in its body.
    pub(super) fn infer_call(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        expected: &Ty<'s>,
    ) -> anyhow::Result<()> {
        for arg in args {
            self.prepare_closure(*arg);
        }
        let pending = DeferredKind::Call { call };
        let prepared = self.prepare_call(call).context("select call signature")?;
        let lookup_input = prepared.is_none().then(|| self.deferred_input(&pending));
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
            self.finish_call(call, prepared, args);
        } else if let Some(lookup_input) = lookup_input {
            // Only lookup needs a body-level retry. Once a call is selected, its remaining
            // predicates and projections are already in the inference table's goal queue.
            if lookup_input != self.deferred_input(&pending) {
                // Ordinary argument inference can refine a local used as receiver or callee.
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

    /// Find a signature before visiting arguments. Ordinary calls use the callee's type.
    /// A dot call selects its function using the receiver.
    pub(super) fn prepare_call(
        &mut self,
        call: ExprId,
    ) -> Result<Option<PreparedCall<'s>>, ReadError> {
        crate::profile::metric::CALL_ATTEMPTS.inc();
        let data = self.body.expr_unchecked(call);
        let prepared = match &data.kind {
            ExprKind::Call { callee, args } => {
                callee.and_then(|callee| self.prepare_value_call(callee, args))
            }
            ExprKind::MethodCall {
                receiver: Some(receiver),
                generic_args,
                ..
            } => {
                let receiver = self.inference.root_resolved_expr_ty(*receiver);
                let targets =
                    self.context
                        .live()
                        .method_targets(call, receiver, self.inference.table())?;
                let Some(target) = LiveCallTarget::unique_usable(targets) else {
                    return Ok(None);
                };
                let receiver = target.receiver;
                self.instantiate_function_target(target, data.scope, generic_args)?
                    .and_then(|item| PreparedCall::from_item(item, receiver, self.cx))
            }
            ExprKind::MethodCall { receiver: None, .. } => None,
            _ => unreachable!("call inference owns a call expression"),
        };
        if let Some(mut prepared) = prepared {
            // Retain the normalized return slot so later evidence can finish its goals.
            prepared.output = self.inference.table().normalize(prepared.output);
            self.inference.set_expr_ty(call, prepared.output);
            return Ok(Some(prepared));
        }
        Ok(None)
    }

    fn prepare_value_call(&self, callee: ExprId, args: &[ExprId]) -> Option<PreparedCall<'s>> {
        let table = self.inference.table();
        let ty = self.inference.root_resolved_expr_ty(callee);
        for callee in table.autoderef(ty) {
            let (params, ret) = match callee.shape() {
                TyShape::FnDef(item) => return PreparedCall::from_item(item, None, self.cx),
                TyShape::Closure(signature) => (signature.params, signature.ret),
                TyShape::FnPointer { params, ret } => (params, ret),
                TyShape::Param(_) | TyShape::Alias(_) | TyShape::Dyn(_) | TyShape::Adt(_) => {
                    let arguments = args
                        .iter()
                        .map(|&arg| self.inference.root_resolved_expr_ty(arg));
                    let Some(signature) = table.callable_bound_signature(callee, arguments) else {
                        continue;
                    };
                    signature
                }
                _ => continue,
            };
            return Some(PreparedCall {
                selected: None,
                params,
                output: ret,
                receiver: None,
            });
        }
        None
    }

    /// A generic function item gets a fresh substitution at each use of its path. For example,
    /// in `let f = id::<_>; f(1u8)`, `f` keeps the same generic argument throughout the body.
    pub(super) fn instantiate_function_target(
        &mut self,
        target: LiveCallTarget<'s>,
        scope: ScopeId,
        explicit_args: &[ItemGenericArg],
    ) -> Result<Option<FnDefTy<'s>>, ReadError> {
        let function = target.function;
        let table = target.table.as_ref().unwrap_or(self.inference.table());
        let item = table.commit_if_some(|table| -> Result<_, ReadError> {
            let generics = self
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Function(function))?;
            let mut subst = target.subst;
            if !explicit_args.is_empty() {
                let args =
                    self.context
                        .live()
                        .generic_args(scope, &generics, explicit_args, table)?;
                for (param, arg) in generics
                    .iter_self()
                    .zip(args.iter().skip(generics.parent_len()))
                {
                    subst.insert(param.param(), arg);
                }
            }
            subst.fresh_for(table, generics.iter().map(|p| p.param()));
            Ok(table.instantiate_function_item(function, &subst))
        })?;
        if item.is_some()
            && let Some(table) = target.table
        {
            self.inference.table.adopt(table);
        }
        Ok(item)
    }

    /// Connect argument results to the prepared signature, then retain the call for finalization.
    /// Bounds can remain pending: later body evidence still reaches their shared variables.
    pub(super) fn finish_call(
        &mut self,
        call: ExprId,
        prepared: PreparedCall<'s>,
        args: &[ExprId],
    ) {
        for (arg, param) in args.iter().zip(prepared.argument_params()) {
            self.coerce_expr_ty(*arg, param);
        }
        if let Some(receiver) = prepared.receiver
            && let Some(param) = prepared.params.first()
        {
            // Lookup retained the adjusted receiver, including any borrow. Relate it to the
            // complete self parameter so a by-value reference receiver keeps its reference.
            self.inference.table().unify(receiver, param);
        }
        let _ = self.inference.table().fulfill();
        if let Some(selected) = prepared.selected {
            self.inference.set_selected_call(call, selected);
        }
    }
}
