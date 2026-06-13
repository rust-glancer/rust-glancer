//! Call-signature inference helpers for body resolution.
//!
//! This layer turns selected call targets into inference constraints without making the main pass
//! know how receiver substitutions and function generic shadows are built.

use rg_ir_model::{
    ExprId, ImplRef, ItemOwner, ScopeId,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{Ty, inference::InferTy};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

use super::{BodyInferenceCtx, InferTypeRefProjector, InferTypeSubst};

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
        let explicit_subst = self.context.generics().subst_for_explicit_args(
            generics,
            explicit_args,
            TypeRefUseSite::Scope(self.context.body().expr_unchecked(call).scope),
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
                inference.instantiate_explicit_type_arg_ty(arg_ty, &resolved_ty);
            used_vars |= arg_used_vars;
            subst.push(inference, param.name.clone(), infer_ty);
        }

        if !used_vars {
            return Ok(false);
        }

        let return_ty =
            InferTypeRefProjector::new(&subst).ty_from_type_ref(ret_ty, resolved_ret_ty);
        inference.set_expr_infer_ty(call, return_ty);
        Ok(true)
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

    /// Use method args to solve receiver vars.
    ///
    /// Example: `values: Vec<?T>; values.push(user)` gives `push(value: T)` evidence `?T = User`.
    pub(crate) fn constrain_receiver_generic_arguments(
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
        self.apply_function_generic_shadows(
            inference,
            &mut subst,
            function_data.signature.generics(),
            target.explicit_args(),
            self.context.body().expr_unchecked(method_call).scope,
        )?;

        let written_params = function_data.signature.params().iter().skip(1);
        let projector = InferTypeRefProjector::new(&subst);
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

        let receiver_ty = inference.expr_ty(receiver);
        subst.bind_type_ref(
            inference,
            &impl_data.self_ty,
            &receiver_ty,
            &impl_data.generics,
        );

        Ok(subst)
    }

    /// Function generics shadow impl generics; `::<User>` then fills function `T`.
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

        for param in &generics.types {
            subst.push(inference, param.name.clone(), InferTy::Unknown);
        }

        let explicit_subst = self.context.generics().subst_for_explicit_args(
            generics,
            explicit_args,
            TypeRefUseSite::Scope(scope),
        )?;
        for param in &generics.types {
            if let Some(ty) = explicit_subst.type_param(param.name.as_str()) {
                subst.push(inference, param.name.clone(), InferTy::from_ty(&ty));
            }
        }

        Ok(())
    }
}
