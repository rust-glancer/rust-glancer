//! Generic substitution helpers for body-aware item projection.
//!
//! Field, associated const, and associated type projection all need the same receiver-driven
//! generic bindings. Explicit generic arguments also produce substitutions, while the type parts
//! inside those arguments still resolve through the type-ref query.

use rg_ir_model::{
    DefMapRef, ImplRef, ItemOwner,
    items::{GenericArg as ItemGenericArg, GenericParams},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{GenericArg, NominalTy, TypeSubst};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

pub(crate) struct BodyGenericsQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyGenericsQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(crate) fn subst_for_nominal_ty(
        &self,
        ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    pub(crate) fn subst_for_receiver_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
        receiver_ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
        let mut subst = self.subst_for_nominal_ty(receiver_ty)?;
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(subst);
        };

        let impl_ref = ImplRef {
            origin,
            id: impl_id,
        };
        if let Some(impl_data) = self.context.item_query().impl_data(impl_ref)? {
            subst.extend(
                self.context
                    .impl_matcher()
                    .impl_self_subst_for_impl(impl_data, receiver_ty),
            );
        }

        Ok(subst)
    }

    pub(crate) fn subst_for_explicit_args(
        &self,
        generics: &GenericParams,
        args: &[ItemGenericArg],
        use_site: TypeRefUseSite,
    ) -> Result<TypeSubst, PackageStoreError> {
        if args.is_empty() {
            return Ok(TypeSubst::new());
        }

        // Explicit arguments can contain type syntax and associated-type bindings, so the
        // generics query owns the substitution while type-ref resolution owns the syntax lowering.
        let arg_resolver = self.context.type_refs(use_site);
        let args = args
            .iter()
            .map(|arg| arg_resolver.resolve_generic_arg(arg))
            .collect::<Result<Vec<GenericArg>, _>>()?;
        Ok(TypeSubst::from_generics(generics, &args))
    }
}
