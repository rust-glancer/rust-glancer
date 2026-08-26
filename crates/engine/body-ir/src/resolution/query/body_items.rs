//! Body-local item lookup for body-aware resolution.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, DefMapRef, FunctionRef, ImplRef, TraitImplRef, TypeDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource};
use rg_std::UniqueVec;
use rg_text::Name;

use crate::resolution::BodyResolutionContext;

/// Finds items declared in bodies, such as local impls and their methods.
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

    /// Return body-local inherent impls whose `Self` resolves to this type.
    pub(super) fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<ImplRef>, PackageStoreError> {
        let mut impls = UniqueVec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_some() || !impl_data.resolved_self_ty.is(&ty) {
                    continue;
                }
                impls.push(impl_ref);
            }
        }

        Ok(impls)
    }

    /// Return body-local inherent functions for this type.
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

    /// Return method names supplied by body-local inherent impls for this type.
    ///
    /// Body-aware lookup combines these impls with a crate-wide index. A local method replaces a
    /// crate-indexed method with the same name inside this body; unrelated saved methods remain
    /// visible.
    pub(super) fn inherent_function_names_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<Name>, PackageStoreError> {
        let item_query = self.context.item_query();
        let mut names = UniqueVec::new();
        for function in self.inherent_functions_for_type(ty)? {
            if let Some(data) = item_query.function_data(function)? {
                names.push(data.name.clone());
            }
        }
        Ok(names)
    }

    /// Return body-local trait impls whose `Self` resolves to this type.
    pub(super) fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = UniqueVec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() || !impl_data.resolved_self_ty.is(&ty) {
                    continue;
                }
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() else {
                    continue;
                };
                trait_impls.push(TraitImplRef {
                    impl_ref,
                    trait_ref: *trait_ref,
                });
            }
        }

        Ok(trait_impls)
    }

    /// Return body-local trait impls whose `Self` type has no nominal receiver key.
    ///
    /// `impl<T> Describe for [T]` cannot be stored under one `TypeDefRef`; the caller collects it
    /// here and later matches its full `[T]` header against the concrete receiver.
    pub(super) fn trait_impls_without_type_key(
        &self,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = UniqueVec::new();

        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() || !impl_data.resolved_self_ty.is_empty() {
                    continue;
                }
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() else {
                    continue;
                };
                trait_impls.push(TraitImplRef {
                    impl_ref,
                    trait_ref: *trait_ref,
                });
            }
        }

        Ok(trait_impls)
    }

    /// Gather body item stores that can affect the current body lookup.
    fn body_lookup_stores(&self) -> Result<Vec<&'query ItemStore>, PackageStoreError> {
        let mut origins = UniqueVec::new();

        // Check the active body first, then the body-local modules that own this declaration and
        // its fallback. Target modules are still handled by CrateItemQuery.
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
