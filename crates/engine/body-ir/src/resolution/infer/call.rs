//! Call-signature inference helpers for body resolution.
//!
//! This layer turns selected call targets into inference constraints without making the main pass
//! know how receiver substitutions and function generic shadows are built.

use rg_ir_model::{
    ExprId, ImplRef, ItemOwner, ScopeId,
    items::{GenericArg as ItemGenericArg, GenericParams},
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
