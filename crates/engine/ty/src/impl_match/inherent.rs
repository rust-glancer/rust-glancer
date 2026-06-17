//! Boolean receiver checks for inherent impls.
//!
//! These checks decide whether an already-selected inherent item can belong to a nominal receiver.
//! They may use impl type and const parameters as wildcards, but concrete args must match.

use crate::{GenericArg, NominalTy, Ty, TypeSubst};
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypeRef};
use rg_ir_model::{FunctionRef, ImplRef, ItemOwner};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};

use super::ImplMatcher;

impl<'query, D, I> ImplMatcher<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    /// Checks whether a function owned by an inherent impl can be called on the receiver type.
    ///
    /// Trait functions are accepted here because the trait impl candidate already carries the
    /// receiver-specific filtering before a trait function reaches this point.
    pub fn function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        // Trait items are shared by all impl candidates in the best-effort model. Inherent impl
        // items, however, must at least match the receiver's resolved self type.
        let item_query = self.item_paths.items();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(false);
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(false);
        };
        self.impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    /// Checks whether an already-loaded inherent impl applies to a nominal receiver.
    ///
    /// Both target Semantic IR and body shadow item stores use `ImplData`, so the low-level
    /// receiver check should not care which store provided the impl.
    pub fn impl_applies_to_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        if !impl_data.resolved_self_ty.is(&receiver_ty.def) {
            return Ok(false);
        }

        self.impl_self_args_match_receiver(impl_ref, impl_data, receiver_ty)
    }

    /// Performs the boolean self-type argument check used by inherent impl methods.
    ///
    /// Type parameters in the impl self type act as wildcards, but concrete arguments must match:
    /// `impl Wrapper<User>` applies to `Wrapper<User>`, not `Wrapper<Project>`.
    fn impl_self_args_match_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        // Type and const parameters in the impl self type act as wildcards. Concrete args such as
        // `impl Wrapper<User>` or `impl Foo<1>` must equal the receiver's known args. Lifetimes do
        // not select inherent impls, so they only need to line up as lifetime arguments.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(true);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(true);
        };

        if segment.args.len() != receiver_ty.args.len() {
            return Ok(false);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let impl_const_params = Self::impl_const_param_names(&impl_data.generics);
        for (impl_arg, receiver_arg) in segment.args.iter().zip(&receiver_ty.args) {
            if !self.impl_self_arg_matches_receiver(
                impl_ref,
                impl_data,
                impl_arg,
                receiver_arg,
                &impl_type_params,
                &impl_const_params,
            )? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn impl_self_arg_matches_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_arg: &ItemGenericArg,
        receiver_arg: &GenericArg,
        impl_type_params: &[&str],
        impl_const_params: &[&str],
    ) -> Result<bool, D::Error> {
        match impl_arg {
            ItemGenericArg::Type(impl_arg) => {
                let Some(receiver_arg) = receiver_arg.as_ty().cloned() else {
                    return Ok(false);
                };
                if impl_arg
                    .type_param_name()
                    .as_deref()
                    .is_some_and(|name| impl_type_params.contains(&name))
                {
                    return Ok(true);
                }

                let context = TypePathContext {
                    module: impl_data.owner,
                    impl_ref: Some(impl_ref),
                };
                let impl_arg_ty = self.item_paths.resolve_type_ref(
                    impl_arg,
                    context,
                    Ty::syntax(impl_arg.clone()),
                    &TypeSubst::new(),
                )?;
                Ok(impl_arg_ty == receiver_arg)
            }
            ItemGenericArg::Lifetime(_) => Ok(matches!(receiver_arg, GenericArg::Lifetime(_))),
            ItemGenericArg::Const(impl_arg) => {
                let GenericArg::Const(receiver_arg) = receiver_arg else {
                    return Ok(false);
                };
                Ok(impl_const_params.contains(&impl_arg.as_str()) || impl_arg == receiver_arg)
            }
            ItemGenericArg::FnTraitArgs { .. }
            | ItemGenericArg::AssocType { .. }
            | ItemGenericArg::Unsupported(_) => Ok(false),
        }
    }
}
