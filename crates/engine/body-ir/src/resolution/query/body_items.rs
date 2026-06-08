//! Body-local item queries that do not fit target-scoped semantic lookup.
//!
//! `TargetItemQuery` models items visible from a target. Impl blocks written in bodies have their
//! headers resolved from body scope, but method lookup treats the resulting impl facts as ordinary
//! impl candidates. This helper keeps that body overlay explicit instead of broadening the
//! target-level semantic index.

use rg_ir_model::{AssocItemId, DefMapRef, FunctionRef, ImplRef, TraitImplRef, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemStore, ItemStoreSource};
use rg_package_store::PackageStoreError;

use crate::resolution::{BodyResolutionContext, support::push_unique};

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
    ) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impls = Vec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_some() || !impl_data.resolved_self_tys.contains(&ty) {
                    continue;
                }
                push_unique(&mut impls, impl_ref);
            }
        }

        Ok(impls)
    }

    pub(super) fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let item_query = self.context.item_query();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            for item in &impl_data.items {
                if let AssocItemId::Function(id) = item {
                    push_unique(
                        &mut functions,
                        FunctionRef {
                            origin: impl_ref.origin,
                            id: *id,
                        },
                    );
                }
            }
        }

        Ok(functions)
    }

    pub(super) fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let item_query = self.context.item_query();
        for function in self.inherent_functions_for_type(ty)? {
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if function_data.name == name {
                push_unique(&mut functions, function);
            }
        }

        Ok(functions)
    }

    pub(super) fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = Vec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() || !impl_data.resolved_self_tys.contains(&ty) {
                    continue;
                }
                for trait_ref in &impl_data.resolved_trait_refs {
                    push_unique(
                        &mut trait_impls,
                        TraitImplRef {
                            impl_ref,
                            trait_ref: *trait_ref,
                        },
                    );
                }
            }
        }

        Ok(trait_impls)
    }

    fn body_lookup_stores(&self) -> Result<Vec<&'query ItemStore>, PackageStoreError> {
        let mut origins = Vec::new();

        // Check the active body first, then the body-local modules that own this declaration and
        // its fallback. Target modules are still handled by TargetItemQuery.
        push_unique(&mut origins, DefMapRef::Body(self.context.body_ref()));
        for module in [
            self.context.body().owner_module(),
            self.context.body().fallback_module(),
        ] {
            if let DefMapRef::Body(_) = module.origin {
                push_unique(&mut origins, module.origin);
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
