//! Call-signature inference over canonical semantic signatures.
//!
//! Call lookup chooses a function and supplies receiver/impl substitutions. This layer gives the
//! selected function's own parameters live inference variables, binds arguments and return
//! evidence, and submits its already-lowered clauses. Declaration syntax is not projected again.

use std::sync::Arc;

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, GenericDefRef, GenericParamRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{Generics, ItemStoreSource};
use rg_ty::{
    Clause, GenericArg, Substitution, TraitProof, Ty,
    inference::{InferenceSubstitution, InferenceTable},
};

use crate::CallFacts;
use crate::resolution::{
    BodyResolutionContext,
    query::{CallProjection, ResolvedCallTarget},
};

use super::BodyInferenceCtx;

/// Call-owned signature projection and live generic slots.
///
/// Fixed-point rounds revisit the same call as new body evidence appears. Keeping its substitution
/// here makes each written `_` and function generic one stable inference variable instead of
/// allocating an unrelated replacement on every pass.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct CallInferenceState {
    function: rg_ir_model::FunctionRef,
    generic_params: Arc<[GenericParamRef]>,
    projection: Arc<CallProjection>,
    subst: InferenceSubstitution,
    first_written_param_idx: usize,
    return_projection_complete: bool,
    generic_obligations_complete: bool,
}

impl CallInferenceState {
    /// Collapse the call-owned inference substitution into its persistent semantic form.
    pub(super) fn finalize(&self, table: &InferenceTable, inference_complete: bool) -> CallFacts {
        let params = self.generic_params.iter().copied();
        let generic_args = if inference_complete {
            self.subst.finalize_args(table, params)
        } else {
            self.subst
                .finalize_args_without_numeric_defaults(table, params)
        };
        CallFacts::new(self.function, generic_args)
    }
}

/// One call's selected state while a fixed-point transfer step applies its evidence.
///
/// Preparing the transfer selects the target at most once. The expression pass can then push the
/// selected parameter types through tuple, array, and reference syntax before completion binds the
/// refined arguments, solves obligations, and stores the call state again.
pub(crate) struct CallInferenceTransfer {
    call: ExprId,
    receiver: Option<ExprId>,
    state: CallInferenceState,
}

impl CallInferenceTransfer {
    /// Return the selected signature's expected types for the written arguments.
    ///
    /// A malformed or incomplete call has no positional correspondence, so an arity mismatch
    /// yields an empty iterator rather than applying expectations to the wrong expressions.
    pub(crate) fn argument_expected_tys<'a>(
        &'a self,
        args: &'a [ExprId],
    ) -> impl Iterator<Item = (ExprId, Ty)> + 'a {
        let written_params = self
            .state
            .projection
            .signature()
            .params
            .get(self.state.first_written_param_idx..)
            .unwrap_or_default();
        (written_params.len() == args.len())
            .then(|| {
                args.iter().copied().zip(
                    written_params
                        .iter()
                        .map(|param| self.state.subst.as_substitution().apply(param)),
                )
            })
            .into_iter()
            .flatten()
    }
}

/// Applies one selected semantic signature to body-owned inference slots.
///
/// Call lookup and signature projection remain query responsibilities. This layer keeps the
/// resulting substitution alive across fixed-point rounds, pushes parameter expectations into the
/// receiver and written arguments, and uses their evidence to refine generics and the return type.
/// Once body inference finishes, `CallInferenceState` collapses to persisted `CallFacts`.
pub(crate) struct BodyCallInference<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyCallInference<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Select one call target and expose its expected argument types for this transfer step.
    ///
    /// Evidence is deliberately bound during `complete_transfer`, after the caller has propagated
    /// these expectations through transparent expression shapes. Selection itself is never
    /// repeated within the step.
    pub(crate) fn prepare_transfer(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<Option<CallInferenceTransfer>, PackageStoreError> {
        let receiver = receiver.or_else(|| match self.context.body().expr_unchecked(call).kind {
            crate::ir::ExprKind::MethodCall { receiver, .. } => receiver,
            _ => None,
        });
        let state = if let Some(state) = inference.call_inference(call) {
            state
        } else {
            let calls = self.context.calls();
            // Keep nested slots in the selected receiver substitution so later arguments can
            // still constrain them. Predicate proof receives the owning table separately and can
            // canonicalize those slots without severing their connection to body inference.
            let receiver_ty = receiver.map(|receiver| inference.root_resolved_expr_ty(receiver));
            let Some(target) =
                calls.target_with_receiver_ty(call, receiver_ty.as_ref(), inference.table())?
            else {
                return Ok(None);
            };
            if let Some(selection) = target.trait_selection() {
                // Candidate proof is transactional until lookup finds one definite target. Commit
                // its table now so equality evidence used to prove the impl remains true while
                // the selected call is refined on later fixed-point rounds.
                inference.table = selection.table.clone();
            }
            let projection = calls.signature(&target).project(args)?;
            let generics = self
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Function(target.function()))?;

            // Allocate call-owned function generics and explicit `_` arguments exactly once.
            // Receiver and argument evidence can keep refining these slots without allocating an
            // unrelated replacement on every fixed-point pass.
            let mut base = projection.subst().clone();
            for param in generics.iter_self() {
                if let GenericParamRef::Type(param) = param.param() {
                    base.push(
                        GenericParamRef::Type(param),
                        GenericArg::Type(Box::new(inference.table.new_type_var())),
                    );
                }
            }
            base.extend(self.explicit_inference_subst(inference, &target, &generics)?);
            CallInferenceState {
                function: target.function(),
                generic_params: generics
                    .iter()
                    .map(|param| param.param())
                    .collect::<Vec<_>>()
                    .into(),
                projection: Arc::new(projection),
                subst: InferenceSubstitution::from_substitution(base),
                first_written_param_idx: target.first_written_param_idx(),
                return_projection_complete: false,
                generic_obligations_complete: false,
            }
        };

        // Install the return shape before argument expectations. Chained calls later in the same
        // expression walk can then use it as receiver evidence.
        if !state.return_projection_complete {
            let return_ty = state
                .subst
                .as_substitution()
                .apply(&state.projection.signature().ret);
            if return_ty.has_var() {
                inference.set_expr_infer_ty(call, return_ty);
            } else if return_ty.has_unknown() {
                inference.instantiate_expr_nested_unknown_ty(call, &return_ty);
            } else {
                inference.set_expr_infer_ty(call, return_ty);
            }
        }

        Ok(Some(CallInferenceTransfer {
            call,
            receiver,
            state,
        }))
    }

    /// Finish one prepared call after its expected types have reached the written arguments.
    pub(crate) fn complete_transfer(
        &self,
        inference: &mut BodyInferenceCtx,
        mut transfer: CallInferenceTransfer,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        // Bind the selected signature once against the evidence refined earlier in this transfer
        // step. Every following operation consumes the same substitution.
        if transfer.state.first_written_param_idx == 1
            && let Some(receiver) = transfer.receiver
            && let Some(receiver_param) = transfer.state.projection.signature().params.first()
        {
            let receiver_pattern = match receiver_param {
                Ty::Reference { inner, .. } => inner.as_ref(),
                receiver_param => receiver_param,
            };
            let receiver_ty = inference.root_resolved_expr_ty(receiver);
            transfer
                .state
                .subst
                .bind_ty(&mut inference.table, receiver_pattern, &receiver_ty);
        }

        for (param, arg) in transfer
            .state
            .projection
            .signature()
            .params
            .iter()
            .skip(transfer.state.first_written_param_idx)
            .zip(args)
        {
            let evidence = inference.root_resolved_expr_ty(*arg);
            transfer
                .state
                .subst
                .bind_ty(&mut inference.table, param, &evidence);
        }
        let return_evidence = inference.expr_ty(transfer.call);
        if !matches!(return_evidence, Ty::Unknown) {
            transfer.state.subst.bind_ty(
                &mut inference.table,
                &transfer.state.projection.signature().ret,
                &return_evidence,
            );
        }

        // Push the now-refined signature back into the written expressions. The outer pass has
        // already handled transparent syntax such as tuple fields; these direct constraints cover
        // partial/malformed calls too and keep receiver autoref handling call-owned.
        for (arg, param) in args.iter().zip(
            transfer
                .state
                .projection
                .signature()
                .params
                .iter()
                .skip(transfer.state.first_written_param_idx),
        ) {
            let expected = transfer.state.subst.as_substitution().apply(param);
            inference.constrain_expr_ty(*arg, &expected);
        }
        if transfer.state.first_written_param_idx == 1
            && let Some(receiver) = transfer.receiver
            && let Some(receiver_param) = transfer.state.projection.signature().params.first()
        {
            // Method-call syntax supplies the receiver before Rust inserts `&self`/`&mut self`.
            let receiver_pattern = match receiver_param {
                Ty::Reference { inner, .. } => inner.as_ref(),
                receiver_param => receiver_param,
            };
            let receiver_ty = transfer
                .state
                .subst
                .as_substitution()
                .apply(receiver_pattern);
            inference.constrain_expr_ty(receiver, &receiver_ty);
        }

        self.solve_generic_trait_obligations(inference, &mut transfer.state)?;
        self.project_selected_trait_associated_return_type(
            inference,
            transfer.call,
            &mut transfer.state,
        )?;
        inference.set_call_inference(transfer.call, transfer.state);
        Ok(())
    }

    /// Submit the selected signature's canonical clauses with live call substitutions applied.
    fn solve_generic_trait_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        state: &mut CallInferenceState,
    ) -> Result<(), PackageStoreError> {
        if state.generic_obligations_complete {
            return Ok(());
        }

        // A selected call obligation matters to this layer while it can still constrain a
        // body-owned inference slot, closure identity, or unresolved semantic shape. Fully settled
        // predicates are type-checking facts; proving them cannot change any Body IR type, so
        // eager indexing should not submit them to Chalk merely to rediscover that the
        // already-selected call is valid.
        //
        // `Unknown` and projections deliberately remain pending. Their producer may become known
        // on a later fixed-point pass, at which point the same obligation can carry useful evidence
        // into the call-owned substitution. Marking either shape complete would freeze calls such
        // as `map(...).collect::<Vec<_>>()` before `Map::Item` has been projected.
        let is_unsettled =
            |ty: &Ty| ty.has_var() || ty.has_closure() || ty.has_unknown() || ty.has_projection();
        let needs_body_inference = state
            .projection
            .signature()
            .clauses
            .iter()
            .map(|clause| state.subst.as_substitution().apply_clause(clause))
            .map(|clause| inference.table.canonicalize_clause(&clause))
            .any(|clause| match clause {
                Clause::Implemented(application) => application
                    .args
                    .iter()
                    .any(|arg| arg.as_ty().is_some_and(is_unsettled)),
                Clause::AliasEq { alias, ty } => {
                    is_unsettled(&ty)
                        || alias
                            .args
                            .iter()
                            .any(|arg| arg.as_ty().is_some_and(is_unsettled))
                }
            });
        if !needs_body_inference {
            state.generic_obligations_complete = true;
            return Ok(());
        }

        let proof = self.context.trait_selection().prove_clauses(
            &state.projection.signature().clauses,
            &state.subst,
            &inference.table,
        )?;
        state.generic_obligations_complete = match proof {
            TraitProof::Proven(table) => {
                inference.table = table;
                true
            }
            TraitProof::Ambiguous(Some(table)) => {
                // Chalk's definite guidance is an equality every possible solution shares. It is
                // safe inference evidence even though it does not prove that the obligation will
                // ultimately hold. Keep the obligation pending and retry after that evidence has
                // propagated through the body.
                inference.table = table;
                false
            }
            TraitProof::Ambiguous(None) | TraitProof::NoSolution | TraitProof::Unavailable => false,
        };
        Ok(())
    }

    /// Normalize associated types in the selected semantic return.
    fn project_selected_trait_associated_return_type(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        state: &mut CallInferenceState,
    ) -> Result<(), PackageStoreError> {
        if state.return_projection_complete {
            return Ok(());
        }
        let return_ty = state
            .subst
            .as_substitution()
            .apply(&state.projection.signature().ret);
        let (ty, table) = self
            .context
            .trait_selection()
            .normalize_ty(&return_ty, &inference.table)?;
        inference.table = table;
        // Once every projection has been replaced, later evidence only solves the inference slots
        // already embedded in this result. Re-running Chalk cannot change its semantic shape.
        state.return_projection_complete = !ty.has_projection();
        if ty != return_ty {
            inference.set_expr_normalized_ty(call, ty);
        } else {
            // A call may become selectable after a preceding transfer step gives its receiver a
            // type. Install the ordinary return shape before marking projection complete.
            inference.set_expr_infer_ty(call, ty);
        }
        Ok(())
    }

    /// Convert written `_` type arguments into inference variables without shifting omitted
    /// lifetime positions.
    fn explicit_inference_subst(
        &self,
        inference: &mut BodyInferenceCtx,
        target: &ResolvedCallTarget,
        generics: &Generics<'_>,
    ) -> Result<Substitution, PackageStoreError> {
        if target.explicit_args().is_empty() {
            return Ok(Substitution::new());
        }
        let resolved = self.context.generics().subst_for_explicit_args(
            GenericDefRef::Function(target.function()),
            target.explicit_args(),
            target.site_scope(),
        )?;
        let positional = target
            .explicit_args()
            .iter()
            .filter(|arg| {
                !matches!(
                    arg,
                    ItemGenericArg::AssocType { .. } | ItemGenericArg::Unsupported(_)
                )
            })
            .collect::<Vec<_>>();
        let mut syntax_index = 0;
        let mut subst = Substitution::new();
        for param in generics.iter_self() {
            let syntax = positional.get(syntax_index).copied();
            match (param.param(), syntax) {
                (GenericParamRef::Lifetime(_), Some(ItemGenericArg::Lifetime(_)))
                | (GenericParamRef::Const(_), Some(ItemGenericArg::Const(_))) => {
                    syntax_index += 1;
                    if let Some(arg) = resolved.get(param.param()) {
                        subst.push(param.param(), arg.clone());
                    }
                }
                // Lifetimes can be omitted without consuming the following type argument.
                (GenericParamRef::Lifetime(_), _) => {}
                (GenericParamRef::Type(type_param), Some(ItemGenericArg::Type(ty))) => {
                    syntax_index += 1;
                    let ty = self
                        .context
                        .type_refs(target.site_scope())
                        .resolve_with_inference(ty, &mut inference.table)?;
                    subst.push(
                        GenericParamRef::Type(type_param),
                        GenericArg::Type(Box::new(ty)),
                    );
                }
                (GenericParamRef::Type(_), Some(ItemGenericArg::FnTraitArgs { .. })) => {
                    syntax_index += 1;
                    if let Some(arg) = resolved.get(param.param()) {
                        subst.push(param.param(), arg.clone());
                    }
                }
                _ => {}
            }
        }
        Ok(subst)
    }
}
