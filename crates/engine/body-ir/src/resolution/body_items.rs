//! Body-local item queries that do not fit target-scoped semantic lookup.
//!
//! `TargetItemQuery` models items visible from a target. Impl blocks written in bodies have their
//! headers resolved from body scope, but method lookup treats the resulting impl facts as ordinary
//! impl candidates. This helper keeps that body overlay explicit instead of broadening the
//! target-level semantic index.

use rg_ir_model::{AssocItemId, DefMapRef, FunctionRef, ImplRef, TraitImplRef, TypeDefRef};
use rg_ir_storage::{ItemStore, ItemStoreQuery, ItemStoreSource};
use rg_package_store::PackageStoreError;

use super::{BodyQuerySource, push_unique};

pub(crate) struct BodyLocalItemQuery<'query, D, I> {
    source: BodyQuerySource<'query, D, I>,
}

impl<'query, D, I> BodyLocalItemQuery<'query, D, I>
where
    D: Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(source: BodyQuerySource<'query, D, I>) -> Self {
        Self { source }
    }

    pub(super) fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impls = Vec::new();
        let Some(store) = self.active_body_store()? else {
            return Ok(impls);
        };

        for (impl_ref, impl_data) in store.impls_with_refs() {
            if impl_data.trait_ref.is_some() || !impl_data.resolved_self_tys.contains(&ty) {
                continue;
            }
            push_unique(&mut impls, impl_ref);
        }

        Ok(impls)
    }

    pub(super) fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let item_query = self.item_query();
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
        let item_query = self.item_query();
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
        let Some(store) = self.active_body_store()? else {
            return Ok(trait_impls);
        };

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

        Ok(trait_impls)
    }

    fn item_query(&self) -> ItemStoreQuery<'query, BodyQuerySource<'query, D, I>> {
        ItemStoreQuery::new(self.source)
    }

    fn active_body_store(&self) -> Result<Option<&'query ItemStore>, PackageStoreError> {
        self.item_query()
            .item_store_for_origin(DefMapRef::Body(self.source.body_ref()))
    }
}
