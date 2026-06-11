//! Body-local item queries that do not fit target-scoped semantic lookup.
//!
//! `TargetItemQuery` models items visible from a target. Impl blocks written in bodies have their
//! headers resolved from body scope, but method lookup treats the resulting impl facts as ordinary
//! impl candidates. This helper keeps that body overlay explicit instead of broadening the
//! target-level semantic index.

use rg_ir_model::{AssocItemId, DefMapRef, FunctionRef, ImplRef, TraitImplRef, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemStore, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;

use crate::resolution::BodyResolutionContext;

pub(crate) struct BodyLocalItemQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyLocalItemQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(super) fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<ImplRef>, PackageStoreError> {
        let mut impls = UniqueVec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_some() || !impl_data.resolved_self_tys.contains(&ty) {
                    continue;
                }
                impls.push(impl_ref);
            }
        }

        Ok(impls)
    }

    pub(super) fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        let mut functions = UniqueVec::new();
        let item_query = self.context.item_query();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            for item in &impl_data.items {
                if let AssocItemId::Function(id) = item {
                    functions.push(FunctionRef {
                        origin: impl_ref.origin,
                        id: *id,
                    });
                }
            }
        }

        Ok(functions)
    }

    pub(super) fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = UniqueVec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() || !impl_data.resolved_self_tys.contains(&ty) {
                    continue;
                }
                for trait_ref in &impl_data.resolved_trait_refs {
                    trait_impls.push(TraitImplRef {
                        impl_ref,
                        trait_ref: *trait_ref,
                    });
                }
            }
        }

        Ok(trait_impls)
    }

    fn body_lookup_stores(&self) -> Result<Vec<&'query ItemStore>, PackageStoreError> {
        let mut origins = UniqueVec::new();

        // Check the active body first, then the body-local modules that own this declaration and
        // its fallback. Target modules are still handled by TargetItemQuery.
        origins.push(DefMapRef::Body(self.context.body_ref()));
        for module in [
            self.context.body().owner_module(),
            self.context.body().fallback_module(),
        ] {
            if let DefMapRef::Body(_) = module.origin {
                origins.push(module.origin);
            }
        }

        let item_query = self.context.item_query();
        let mut stores = Vec::new();
        for origin in origins {
            if let Some(store) = item_query.item_store_for_origin(origin)? {
                stores.push(store);
            }
        }
        Ok(stores)
    }
}
