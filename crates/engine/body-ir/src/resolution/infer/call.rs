//! Call-signature inference over canonical semantic signatures.
//!
//! Call lookup chooses a function and supplies receiver/impl substitutions. This layer gives the
//! selected function's own parameters live inference variables, binds arguments and return
//! evidence, and submits its already-lowered clauses. Declaration syntax is not projected again.

use std::sync::Arc;

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ExprId, GenericDefRef, GenericParamRef, SemanticItemRef, identity::DeclarationRef,
    items::GenericArg as ItemGenericArg,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{Generics, ItemStoreSource};
use rg_ty::{Clause, GenericArg, Substitution, Ty, inference::InferenceSubstitution};

use crate::ir::resolved::BodyResolution;
use crate::resolution::{
    BodyResolutionContext, TypeRefUseSite,
    query::{CallProjection, ResolvedCallTarget},
};

use super::{BodyInferenceCtx, trait_obligation::BodyTraitObligationSolver};

/// Call-owned signature projection and live generic slots.
///
/// Several inference stages revisit the same call. Keeping their substitution here makes each
/// written `_` and function generic one stable inference variable instead of allocating an
/// unrelated replacement on every fixed-point pass.
#[derive(Clone)]
pub(super) struct CallInferenceState {
    projection: Arc<CallProjection>,
    subst: InferenceSubstitution,
    first_written_param_idx: usize,
    return_projection_complete: bool,
    generic_obligations_complete: bool,
}

/// Bridges selected semantic call signatures into body inference constraints.
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

    /// Instantiate and store the selected call's semantic return type.
    pub(crate) fn instantiate_return_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let Some(call_inference) = self.selected_call_inference(inference, call, args, receiver)?
        else {
            return Ok(());
        };
        if call_inference.return_projection_complete {
            return Ok(());
        }
        let return_ty = call_inference
            .subst
            .as_substitution()
            .apply(&call_inference.projection.signature().ret);
        if return_ty.has_var() {
            inference.set_expr_infer_ty(call, return_ty);
        } else if return_ty.has_unknown() {
            inference.instantiate_expr_nested_unknown_ty(call, &return_ty);
        } else {
            inference.set_expr_infer_ty(call, return_ty);
        }
        Ok(())
    }

    /// Return expected types for arguments of the unique selected target.
    pub(crate) fn argument_expected_tys(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<Vec<(ExprId, Ty)>, PackageStoreError> {
        let Some(call_inference) = self.selected_call_inference(inference, call, args, None)?
        else {
            return Ok(Vec::new());
        };
        let written_params = call_inference
            .projection
            .signature()
            .params
            .get(call_inference.first_written_param_idx..)
            .unwrap_or_default();
        if written_params.len() != args.len() {
            return Ok(Vec::new());
        }
        Ok(args
            .iter()
            .copied()
            .zip(
                written_params
                    .iter()
                    .map(|param| call_inference.subst.as_substitution().apply(param)),
            )
            .collect())
    }

    /// Normalize associated types in the selected semantic return.
    pub(crate) fn project_selected_trait_associated_return_type(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let Some(mut call_inference) =
            self.selected_call_inference(inference, call, args, receiver)?
        else {
            return Ok(());
        };
        if call_inference.return_projection_complete {
            return Ok(());
        }
        let return_ty = call_inference
            .subst
            .as_substitution()
            .apply(&call_inference.projection.signature().ret);
        let ty =
            BodyTraitObligationSolver::new(self.context).normalize_ty(inference, &return_ty)?;
        // Once every projection has been replaced, later evidence only solves the inference slots
        // already embedded in this result. Re-running Chalk cannot change its semantic shape.
        call_inference.return_projection_complete = !ty.has_projection();
        inference.set_expr_infer_ty(call, ty);
        inference.set_call_inference(call, call_inference);
        Ok(())
    }

    /// Bind call arguments to function-owned parameters and constrain their expression slots.
    pub(crate) fn constrain_function_generic_arguments(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let Some(call_inference) = self.selected_call_inference(inference, call, args, None)?
        else {
            return Ok(());
        };
        for (arg, param) in args.iter().zip(
            call_inference
                .projection
                .signature()
                .params
                .iter()
                .skip(call_inference.first_written_param_idx),
        ) {
            let expected = call_inference.subst.as_substitution().apply(param);
            inference.constrain_expr_ty(*arg, &expected);
        }
        Ok(())
    }

    /// Constrain the receiver and written arguments from the same instantiated signature.
    pub(crate) fn constrain_selected_method_receiver_and_arguments(
        &self,
        inference: &mut BodyInferenceCtx,
        method_call: ExprId,
        receiver: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let Some(call_inference) =
            self.selected_call_inference(inference, method_call, args, Some(receiver))?
        else {
            return Ok(());
        };
        let Some(receiver_param) = call_inference.projection.signature().params.first() else {
            return Ok(());
        };
        // Method-call syntax supplies the receiver value before Rust inserts the `&self` or
        // `&mut self` autoref. Constrain that value against the inner canonical receiver shape;
        // arbitrary owned receivers continue to use their full declared type.
        let receiver_pattern = match receiver_param {
            Ty::Reference { inner, .. } => inner.as_ref(),
            receiver_param => receiver_param,
        };
        let receiver_ty = call_inference
            .subst
            .as_substitution()
            .apply(receiver_pattern);
        inference.constrain_expr_ty(receiver, &receiver_ty);
        for (arg, param) in args
            .iter()
            .zip(call_inference.projection.signature().params.iter().skip(1))
        {
            let expected = call_inference.subst.as_substitution().apply(param);
            inference.constrain_expr_ty(*arg, &expected);
        }
        Ok(())
    }

    /// Submit the selected signature's canonical clauses with live call substitutions applied.
    pub(crate) fn solve_generic_trait_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let Some(mut call_inference) =
            self.selected_call_inference(inference, call, args, receiver)?
        else {
            return Ok(());
        };
        if call_inference.generic_obligations_complete {
            return Ok(());
        }

        // A selected call obligation matters to this layer only when it can still constrain a
        // body-owned inference slot or closure witness. Fully concrete predicates are type-checking
        // facts; proving them cannot change any Body IR type, so eager indexing should not submit
        // them to Chalk merely to rediscover that the already-selected call is valid.
        let needs_body_inference = call_inference
            .projection
            .signature()
            .clauses
            .iter()
            .map(|clause| call_inference.subst.as_substitution().apply_clause(clause))
            .map(|clause| inference.table.canonicalize_clause(&clause))
            .any(|clause| match clause {
                Clause::Implemented(application) => application.args.iter().any(|arg| {
                    arg.as_ty()
                        .is_some_and(|ty| ty.has_var() || ty.has_closure())
                }),
                Clause::AliasEq { alias, ty } => {
                    ty.has_var()
                        || ty.has_closure()
                        || alias.args.iter().any(|arg| {
                            arg.as_ty()
                                .is_some_and(|ty| ty.has_var() || ty.has_closure())
                        })
                }
            });
        if !needs_body_inference {
            call_inference.generic_obligations_complete = true;
            inference.set_call_inference(call, call_inference);
            return Ok(());
        }

        call_inference.generic_obligations_complete = BodyTraitObligationSolver::new(self.context)
            .solve_selected_call(
                inference,
                &call_inference.projection.signature().clauses,
                &call_inference.subst,
            )?;
        inference.set_call_inference(call, call_inference);
        Ok(())
    }

    /// Reuse the one live signature projection and substitution owned by this call expression.
    fn selected_call_inference(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<Option<CallInferenceState>, PackageStoreError> {
        let receiver = receiver.or_else(|| match self.context.body().expr_unchecked(call).kind {
            crate::ir::ExprKind::MethodCall { receiver, .. } => receiver,
            _ => None,
        });
        let mut call_inference = if let Some(call_inference) = inference.call_inference(call) {
            call_inference
        } else {
            let calls = self.context.calls();
            let receiver_ty = receiver.map(|receiver| inference.root_resolved_expr_ty(receiver));
            let Some(target) = calls.target_with_receiver_ty(call, receiver_ty.as_ref())? else {
                return Ok(None);
            };
            let projection = calls.signature(&target).project(args)?;
            let generics = self
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Function(target.function()))?;

            // Allocate call-owned function generics and explicit `_` arguments exactly once.
            // Receiver and argument evidence below can keep refining these same slots on every
            // fixed-point pass without growing a new alias chain through the inference table.
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
                projection: Arc::new(projection),
                subst: InferenceSubstitution::from_substitution(base),
                first_written_param_idx: target.first_written_param_idx(),
                return_projection_complete: false,
                generic_obligations_complete: false,
            }
        };

        if call_inference.first_written_param_idx == 1
            && let Some(receiver) = receiver
            && let Some(receiver_param) = call_inference.projection.signature().params.first()
        {
            // Bind impl/trait parameters from the live receiver before projecting the return.
            // The body expression precedes method autoref, so `&mut Vec<T>` matches `Vec<?T>`
            // through its inner type while an owned `self: Box<Self>` keeps the full pattern.
            let receiver_pattern = match receiver_param {
                Ty::Reference { inner, .. } => inner.as_ref(),
                receiver_param => receiver_param,
            };
            let receiver_ty = inference.root_resolved_expr_ty(receiver);
            call_inference
                .subst
                .bind_ty(&mut inference.table, receiver_pattern, &receiver_ty);
        }

        for (param, arg) in call_inference
            .projection
            .signature()
            .params
            .iter()
            .skip(call_inference.first_written_param_idx)
            .zip(args)
        {
            let evidence = self
                .fn_def_arg_ty(*arg)?
                .unwrap_or_else(|| inference.root_resolved_expr_ty(*arg));
            call_inference
                .subst
                .bind_ty(&mut inference.table, param, &evidence);
        }
        let return_evidence = inference.expr_ty(call);
        if !matches!(return_evidence, Ty::Unknown) {
            call_inference.subst.bind_ty(
                &mut inference.table,
                &call_inference.projection.signature().ret,
                &return_evidence,
            );
        }

        inference.set_call_inference(call, call_inference.clone());
        Ok(Some(call_inference))
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
            TypeRefUseSite::Scope(target.site_scope()),
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
                        .type_refs(TypeRefUseSite::Scope(target.site_scope()))
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

    fn fn_def_arg_ty(&self, arg: ExprId) -> Result<Option<Ty>, PackageStoreError> {
        let BodyResolution::Declarations(declarations) = self.context.body().expr_resolution(arg)
        else {
            return Ok(None);
        };
        let Some(DeclarationRef::Item(SemanticItemRef::Function(function))) = declarations.as_one()
        else {
            return Ok(None);
        };
        self.context.signatures().function_ty(*function)
    }
}
