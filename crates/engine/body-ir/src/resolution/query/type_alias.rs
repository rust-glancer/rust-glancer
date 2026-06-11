//! Type alias projection.

use rg_ir_model::{AssocItemId, TypeAliasRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_ty::{GenericArg, NominalTy, Ty, TypeSubst};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

/// Projects type aliases into concrete types.
///
/// Handles generic args and receiver substitutions.
pub(crate) struct BodyTypeAliasQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTypeAliasQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Apply alias generics when exactly one alias was resolved.
    pub(crate) fn ty_from_aliases(
        &self,
        aliases: &[TypeAliasRef],
        args: &[GenericArg],
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        if aliases.len() != 1 {
            return Ok(Ty::Unknown);
        }

        self.ty_from_alias(
            aliases
                .first()
                .copied()
                .expect("one alias should exist after length check"),
            args,
            subst,
        )
    }

    /// Find an associated type alias with this name for the given type.
    pub(crate) fn associated_alias_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
        let impls = self
            .context
            .body_local_items()
            .inherent_impls_for_type(ty.def)?;

        let item_query = self.context.item_query();
        for impl_ref in impls {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if !self
                .context
                .impl_matcher()
                .impl_applies_to_receiver(impl_ref, impl_data, ty)?
            {
                continue;
            }

            for item in &impl_data.items {
                let AssocItemId::TypeAlias(id) = item else {
                    continue;
                };
                let alias_ref = TypeAliasRef {
                    origin: impl_ref.origin,
                    id: *id,
                };
                let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
                    continue;
                };
                if alias_data.name == name {
                    return Ok(Some(alias_ref));
                }
            }
        }

        Ok(None)
    }

    /// Project an associated alias using receiver substitutions.
    pub(crate) fn ty_from_associated_alias(
        &self,
        alias_ref: TypeAliasRef,
        receiver_ty: &NominalTy,
        args: &[GenericArg],
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(Ty::Unknown);
        };
        if aliased_ty.is_self_type() {
            return Ok(Ty::nominal([receiver_ty.clone()].into_iter().collect()));
        }

        let mut alias_subst = self.context.generics().subst_for_receiver_owner(
            alias_ref.origin,
            alias_data.owner,
            receiver_ty,
        )?;
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(TypeSubst::from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        self.context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .with_subst(&alias_subst)
            .resolve(aliased_ty)
    }

    /// Project one ordinary type alias into a type.
    fn ty_from_alias(
        &self,
        alias_ref: TypeAliasRef,
        args: &[GenericArg],
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(Ty::Unknown);
        };
        if aliased_ty.is_self_type() {
            return Ok(Ty::Unknown);
        }

        let mut alias_subst = subst.clone();
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(TypeSubst::from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        self.context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .with_subst(&alias_subst)
            .resolve(aliased_ty)
    }
}
