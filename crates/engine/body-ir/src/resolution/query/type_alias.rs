//! Type alias projection.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, DefMapRef, ImplRef, TypeAliasRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::AdtTy;

use crate::resolution::BodyResolutionContext;

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

    /// Find an associated type alias with this name for the given type.
    pub(crate) fn associated_alias_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
        // Block-local impls can add aliases even to crate-origin types, e.g.
        // `impl TargetType { type LocalAlias = ... }` inside a function.
        let body_alias = self.associated_alias_for_impls(
            self.context
                .body_local_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )?;

        // If type originates in body or we already have response, we don't need
        // to check semantic items.
        if matches!(ty.def.origin, DefMapRef::Body(_)) || body_alias.is_some() {
            return Ok(body_alias);
        }

        self.associated_alias_for_impls(
            self.context
                .semantic_index()
                .inherent_impls_for_type(ty.def),
            ty,
            name,
        )
    }

    /// Find the first matching associated type alias across inherent impls.
    fn associated_alias_for_impls(
        &self,
        impls: UniqueVec<ImplRef>,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
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
}
