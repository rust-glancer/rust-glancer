//! Type alias projection.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, TypeAliasRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{AdtTy, Ty};

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
        let receiver_ty = Ty::adt(ty.clone());
        let receiver = self
            .context
            .impls()
            .inherent_matches_for_receiver(&receiver_ty)?;
        let item_query = self.context.item_query();
        for impl_match in receiver.matches().inherent() {
            let Some(impl_data) = item_query.impl_data(impl_match.impl_ref())? else {
                continue;
            };

            for item in &impl_data.items {
                let AssocItemId::TypeAlias(id) = item else {
                    continue;
                };
                let alias_ref = TypeAliasRef {
                    origin: impl_match.impl_ref().origin,
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
