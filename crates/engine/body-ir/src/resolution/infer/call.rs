//! Call-signature inference helpers for body resolution.
//!
//! This layer turns selected call targets into inference constraints without making the main pass
//! know how receiver substitutions and function generic shadows are built.

use rg_ir_model::{
    DefMapRef, ExprId, ImplRef, ItemOwner, ScopeId,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_ty::{
    Ty, TypeSubst,
    inference::{InferTy, InferTypeRefProjector, InferTypeSubst},
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

use super::{
    BodyInferenceCtx,
    trait_obligation::{BodyTraitObligationSolver, SelectedCallObligationInput},
};

/// Bridges selected call signatures into inference constraints.
///
/// It asks call resolution for one target, then maps signature facts back to written args.
pub(crate) struct BodyCallInference<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyCallInference<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Build call inference from a read-only body resolution context.
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Instantiate one call return, e.g. `Vec::new()` from `Vec<unknown>` to `Vec<?T>`.
    pub(crate) fn instantiate_return_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        if !self.context.body().expr_ty_unchecked(call).has_unknown() {
            return Ok(());
        }

        let calls = self.context.calls();
        let Some(target) = calls.target(call)? else {
            return Ok(());
        };
        let projection = calls.signature(&target).project(args)?;

        let mut instantiated = false;
        if !projection.explicit_args().is_empty()
            && let Some(ret_ty) = projection.declared_return_ty()
            && let Some(generics) = projection.function_generics()
        {
            instantiated = self.instantiate_explicit_type_arg_return_fact(
                inference,
                call,
                ret_ty,
                projection.return_ty(),
                generics,
                projection.explicit_args(),
            )?;
        }

        if projection.explicit_args().is_empty()
            && let Some(ret_ty) = projection.declared_return_ty()
            && let Some(generics) = projection.function_generics()
        {
            let type_params = generics
                .types
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>();
            if ret_ty.mentions_type_param(&type_params) {
                instantiated = inference.instantiate_expr_generic_return_ty(
                    call,
                    ret_ty,
                    projection.return_ty(),
                    generics,
                );
            }
        }

        if !instantiated
            && projection.selected_self_ty().is_some_and(Ty::has_unknown)
            && projection.return_ty().has_unknown()
        {
            inference.instantiate_expr_nested_unknown_ty(call, projection.return_ty());
        }

        Ok(())
    }

    /// Instantiate explicit `_` args before projecting the call return.
    fn instantiate_explicit_type_arg_return_fact(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        ret_ty: &TypeRef,
        resolved_ret_ty: &Ty,
        generics: &GenericParams,
        explicit_args: &[ItemGenericArg],
    ) -> Result<bool, PackageStoreError> {
        let scope = self.context.body().expr_unchecked(call).scope;
        let (subst, used_vars) =
            self.explicit_type_arg_infer_subst(inference, generics, explicit_args, scope)?;

        if !used_vars {
            return Ok(false);
        }

        let return_ty =
            InferTypeRefProjector::new(&subst).ty_from_type_ref(ret_ty, resolved_ret_ty);
        inference.set_expr_infer_ty(call, return_ty);
        Ok(true)
    }

    /// Bind explicit type args, turning written `_` into inference vars.
    fn explicit_type_arg_infer_subst(
        &self,
        inference: &mut BodyInferenceCtx,
        generics: &GenericParams,
        explicit_args: &[ItemGenericArg],
        scope: ScopeId,
    ) -> Result<(InferTypeSubst, bool), PackageStoreError> {
        let explicit_subst = self.context.generics().subst_for_explicit_args(
            generics,
            explicit_args,
            TypeRefUseSite::Scope(scope),
        )?;
        let mut explicit_type_args = explicit_args.iter().filter_map(ItemGenericArg::type_ref);

        let mut subst = InferTypeSubst::new();
        let mut used_vars = false;
        for param in &generics.types {
            let Some(arg_ty) = explicit_type_args.next() else {
                break;
            };
            let Some(resolved_ty) = explicit_subst.type_param(param.name.as_str()) else {
                continue;
            };

            let (infer_ty, arg_used_vars) =
                inference.instantiate_written_infer_ty(arg_ty, &resolved_ty);
            used_vars |= arg_used_vars;
            subst.push(&mut inference.table, param.name.clone(), infer_ty);
        }

        Ok((subst, used_vars))
    }

    /// Return expected types for written args from the unique selected call target.
    pub(crate) fn argument_expected_tys(
        &self,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<Vec<(ExprId, Ty)>, PackageStoreError> {
        // Only a single resolved function gives us trustworthy parameter evidence. Ambiguous calls
        // keep their already-computed return type but do not push expectations inward.
        let calls = self.context.calls();
        let Some(target) = calls.target(call)? else {
            return Ok(Vec::new());
        };
        let projection = calls.signature(&target).project(args)?;
        if projection.written_param_tys().len() != args.len() {
            return Ok(Vec::new());
        }

        Ok(args
            .iter()
            .copied()
            .zip(projection.written_param_tys().iter().cloned())
            .collect())
    }

    /// Use call args to solve function generics shared with the call result.
    ///
    /// Example: `id(missing())` makes the arg and return share the same `?T`.
    pub(crate) fn constrain_function_generic_arguments(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let calls = self.context.calls();
        let Some(target) = calls.target(call)? else {
            return Ok(());
        };
        let Some(function_data) = self
            .context
            .item_query()
            .function_data(target.function())?
            .cloned()
        else {
            return Ok(());
        };
        let projection = calls.signature(&target).project(args)?;
        if projection.written_param_tys().len() != args.len() {
            return Ok(());
        }

        let scope = self.context.body().expr_unchecked(call).scope;
        let mut subst = self.type_prefix_impl_infer_subst(
            inference,
            call,
            target.has_type_prefix_self_source(),
            target.function().origin,
            &function_data.owner,
            function_data.signature.ret_ty(),
        )?;
        self.apply_function_generic_shadows(
            inference,
            &mut subst,
            function_data.signature.generics(),
            target.explicit_args(),
            scope,
        )?;

        if let Some(generics) = function_data.signature.generics()
            && let Some(ret_ty) = function_data.signature.ret_ty()
        {
            let return_ty = inference.expr_ty(call);
            subst.bind_type_ref(&mut inference.table, ret_ty, &return_ty, generics);
        }

        let written_params = function_data
            .signature
            .params()
            .iter()
            .skip(target.first_written_param_idx());
        let mut projector = InferTypeRefProjector::new(&subst);
        for ((arg, param), resolved_ty) in args
            .iter()
            .zip(written_params)
            .zip(projection.written_param_tys())
        {
            let Some(param_ty) = &param.ty else {
                continue;
            };
            let expected_ty = projector.ty_from_type_ref(param_ty, resolved_ty);
            inference.constrain_expr_infer_ty(*arg, &expected_ty);
        }

        Ok(())
    }

    /// Bind impl generics for a static `Type::function` call from its result slot.
    ///
    /// Example: `Vec::singleton(user): Vec<?T>` gives `impl<T> Vec<T>` evidence `T = ?T`.
    fn type_prefix_impl_infer_subst(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        has_type_prefix_self_source: bool,
        origin: rg_ir_model::DefMapRef,
        owner: &ItemOwner,
        ret_ty: Option<&TypeRef>,
    ) -> Result<InferTypeSubst, PackageStoreError> {
        let mut subst = InferTypeSubst::new();
        if !has_type_prefix_self_source {
            return Ok(subst);
        }

        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(subst);
        };

        let impl_ref = ImplRef {
            origin,
            id: *impl_id,
        };
        let Some(impl_data) = self.context.item_query().impl_data(impl_ref)?.cloned() else {
            return Ok(subst);
        };

        let return_ty = inference.root_resolved_expr_ty(call);
        subst.bind_type_ref(
            &mut inference.table,
            &impl_data.self_ty,
            &return_ty,
            &impl_data.generics,
        );
        if let Some(ret_ty) = ret_ty {
            subst.bind_type_ref(
                &mut inference.table,
                ret_ty,
                &return_ty,
                &impl_data.generics,
            );
        }

        Ok(subst)
    }

    /// Use a selected method to solve receiver vars.
    ///
    /// Examples:
    ///
    /// - `values: Vec<?T>; values.push(user)` gives `push(value: T)` evidence `?T = User`.
    /// - `wrapper: Wrapper<?T>; wrapper.touch()` selected from `impl Wrapper<User>` gives
    ///   receiver evidence `?T = User`.
    pub(crate) fn constrain_selected_method_receiver_and_arguments(
        &self,
        inference: &mut BodyInferenceCtx,
        method_call: ExprId,
        receiver: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let calls = self.context.calls();
        let Some(target) = calls.target(method_call)? else {
            return Ok(());
        };
        let Some(function_data) = self
            .context
            .item_query()
            .function_data(target.function())?
            .cloned()
        else {
            return Ok(());
        };
        if !function_data.has_self_receiver() {
            return Ok(());
        }

        let projection = calls.signature(&target).project(args)?;
        if projection.written_param_tys().len() != args.len() {
            return Ok(());
        }

        let mut subst = self.receiver_infer_subst(
            inference,
            target.function().origin,
            &function_data.owner,
            receiver,
        )?;
        self.constrain_selected_inherent_method_receiver(
            inference,
            target.function().origin,
            &function_data.owner,
            receiver,
            &subst,
        )?;
        self.apply_function_generic_shadows(
            inference,
            &mut subst,
            function_data.signature.generics(),
            target.explicit_args(),
            self.context.body().expr_unchecked(method_call).scope,
        )?;

        let written_params = function_data.signature.params().iter().skip(1);
        let mut projector = InferTypeRefProjector::new(&subst);
        for ((arg, param), resolved_ty) in args
            .iter()
            .zip(written_params)
            .zip(projection.written_param_tys())
        {
            let Some(param_ty) = &param.ty else {
                continue;
            };
            let expected_ty = projector.ty_from_type_ref(param_ty, resolved_ty);
            inference.constrain_expr_infer_ty(*arg, &expected_ty);
        }

        Ok(())
    }

    /// Push concrete `impl Self` evidence into the receiver slot for selected inherent methods.
    fn constrain_selected_inherent_method_receiver(
        &self,
        inference: &mut BodyInferenceCtx,
        origin: DefMapRef,
        owner: &ItemOwner,
        receiver: ExprId,
        subst: &InferTypeSubst,
    ) -> Result<(), PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(());
        };

        let impl_ref = ImplRef {
            origin,
            id: *impl_id,
        };
        let Some(impl_data) = self.context.item_query().impl_data(impl_ref)?.cloned() else {
            return Ok(());
        };
        if impl_data.trait_ref.is_some() {
            return Ok(());
        }

        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let resolved_self_ty = self.context.item_paths().resolve_type_ref(
            &impl_data.self_ty,
            context,
            Ty::syntax(impl_data.self_ty.clone()),
            &TypeSubst::new(),
        )?;
        let receiver_evidence = InferTypeRefProjector::new(subst)
            .ty_from_type_ref(&impl_data.self_ty, &resolved_self_ty);
        let receiver_ty = inference.root_resolved_expr_ty(receiver);

        // Method lookup may have used autoderef while the source receiver expression still has its
        // pre-adjustment type. Only commit the self-type evidence when it fits that original slot.
        let mut trial_table = inference.table.clone();
        if trial_table
            .try_unify(&receiver_ty, &receiver_evidence)
            .is_err()
        {
            return Ok(());
        }

        inference.constrain_expr_infer_ty(receiver, &receiver_evidence);
        Ok(())
    }

    /// Solve shallow trait bounds on already-selected generic calls.
    ///
    /// Example: `collect::<Vec<_>>()` produces `B = Vec<?T>` from the return type and then solves
    /// the selected function bound `B: FromIterator<Item>` through visible impls.
    pub(crate) fn solve_generic_trait_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let calls = self.context.calls();
        let Some(target) = calls.target(call)? else {
            return Ok(());
        };
        let Some(function_data) = self
            .context
            .item_query()
            .function_data(target.function())?
            .cloned()
        else {
            return Ok(());
        };
        let Some(generics) = function_data.signature.generics() else {
            return Ok(());
        };
        if generics.types.iter().all(|param| param.bounds.is_empty())
            && generics.where_predicates.is_empty()
        {
            return Ok(());
        }

        // Stage 1: rebuild the substitution that connects signature names to inference slots.
        // Return evidence is especially important for `collect::<Vec<_>>()`: it turns `B` into
        // the already-instantiated destination shape `Vec<?T>`.
        let projection = calls.signature(&target).project(args)?;
        let mut subst = self.type_prefix_impl_infer_subst(
            inference,
            call,
            target.has_type_prefix_self_source(),
            target.function().origin,
            &function_data.owner,
            function_data.signature.ret_ty(),
        )?;
        if let Some(receiver) = receiver {
            subst = self.receiver_infer_subst(
                inference,
                target.function().origin,
                &function_data.owner,
                receiver,
            )?;
        }
        self.apply_function_generic_shadows(
            inference,
            &mut subst,
            Some(generics),
            target.explicit_args(),
            self.context.body().expr_unchecked(call).scope,
        )?;

        if let Some(ret_ty) = function_data.signature.ret_ty() {
            let return_ty = inference.expr_ty(call);
            subst.bind_type_ref(&mut inference.table, ret_ty, &return_ty, generics);
        }

        // Stage 2+: lower selected-call bounds into trait goals and commit only unique solutions.
        BodyTraitObligationSolver::new(self.context).solve_selected_call(
            inference,
            SelectedCallObligationInput {
                function: target.function(),
                owner: function_data.owner,
                generics,
                subst: &subst,
                signature_subst: projection.subst(),
                selected_self_ty: projection.selected_self_ty(),
            },
        )
    }

    /// Bind impl generics from the selected receiver slot: `impl<T> Vec<T>` + `Vec<?T>`.
    fn receiver_infer_subst(
        &self,
        inference: &mut BodyInferenceCtx,
        origin: rg_ir_model::DefMapRef,
        owner: &ItemOwner,
        receiver: ExprId,
    ) -> Result<InferTypeSubst, PackageStoreError> {
        let mut subst = InferTypeSubst::new();
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(subst);
        };

        let impl_ref = ImplRef {
            origin,
            id: *impl_id,
        };
        let Some(impl_data) = self.context.item_query().impl_data(impl_ref)?.cloned() else {
            return Ok(subst);
        };

        let receiver_ty = inference.root_resolved_expr_ty(receiver);
        subst.bind_type_ref(
            &mut inference.table,
            &impl_data.self_ty,
            &receiver_ty,
            &impl_data.generics,
        );

        Ok(subst)
    }

    /// Function generics shadow impl generics; `::<User>` or return evidence then fills `T`.
    fn apply_function_generic_shadows(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &mut InferTypeSubst,
        generics: Option<&GenericParams>,
        explicit_args: &[ItemGenericArg],
        scope: ScopeId,
    ) -> Result<(), PackageStoreError> {
        let Some(generics) = generics else {
            return Ok(());
        };

        subst.shadow_type_params(&mut inference.table, generics);

        let explicit_subst = self.context.generics().subst_for_explicit_args(
            generics,
            explicit_args,
            TypeRefUseSite::Scope(scope),
        )?;
        for param in &generics.types {
            if let Some(ty) = explicit_subst.type_param(param.name.as_str()) {
                subst.push(
                    &mut inference.table,
                    param.name.clone(),
                    InferTy::from_ty(&ty),
                );
            }
        }

        Ok(())
    }
}
