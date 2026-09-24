//! A small declaration index for items supplied by the active body view.
//!
//! Persisted Semantic IR indexes describe the last saved crate snapshot. Body resolution may also
//! see declarations from an active source overlay or from a body-local module:
//!
//! ```text
//! fn inspect() {
//!     struct Local;
//!     impl Local { fn run(&self) {} }
//!     Local.run();
//! }
//! ```
//!
//! Those declarations cannot be added to the persisted crate index. The first body query scans the
//! few body item stores that can affect this body and builds a compact overlay index. Later
//! queries read the same index through [`BodyLocalItemCache`](crate::resolution::cache::BodyLocalItemCache).

use std::collections::HashSet;

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, DefMapRef, ImplRef, TraitDefRef, TraitImplRef, TypeDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource};
use rg_std::UniqueVec;

use crate::resolution::{
    BodyResolutionContext,
    cache::{BodyLocalInherentItemNames, BodyLocalItemIndex},
};

/// Read adapter for declarations that exist only in the current body view.
///
/// Query methods return borrowed slices from the lazily built overlay. Callers then combine them
/// with persisted project candidates without cloning the whole body-local collection.
pub(crate) struct BodyLocalItemQuery<'context, 'query, D, I> {
    context: &'context BodyResolutionContext<'query, D, I>,
}

impl<'context, 'query, D, I> BodyLocalItemQuery<'context, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: &'context BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return body-local inherent impls whose `Self` resolves to this type.
    pub(super) fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<&[ImplRef], PackageStoreError> {
        Ok(self
            .index()?
            .inherent_impls_by_type
            .get(&ty)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return names supplied by body-local inherent impls for this type.
    ///
    /// Body-aware lookup combines these impls with a crate-wide index. A current declaration
    /// replaces a crate-indexed declaration of the same kind and name; unrelated saved members
    /// remain visible.
    pub(super) fn inherent_item_names_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&BodyLocalInherentItemNames>, PackageStoreError> {
        Ok(self.index()?.inherent_item_names_by_type.get(&ty))
    }

    /// Return body-local trait impls whose `Self` resolves to this type.
    pub(super) fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<&[TraitImplRef], PackageStoreError> {
        Ok(self
            .index()?
            .trait_impls_by_type
            .get(&ty)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local impls for already-selected traits in one store pass.
    pub(crate) fn trait_impls_for_traits(
        &self,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<impl Iterator<Item = TraitImplRef> + '_, PackageStoreError> {
        let trait_refs = trait_refs.into_iter().collect::<HashSet<_>>();
        Ok(self
            .index()?
            .trait_impls
            .iter()
            .filter(move |candidate| trait_refs.contains(&candidate.trait_ref))
            .copied())
    }

    /// Return body-local traits declaring a function with this name.
    pub(super) fn traits_with_function_name(
        &self,
        name: &str,
    ) -> Result<&[TraitDefRef], PackageStoreError> {
        Ok(self
            .index()?
            .traits_by_function_name
            .get(name)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local traits declaring an associated const with this name.
    pub(super) fn traits_with_const_name(
        &self,
        name: &str,
    ) -> Result<&[TraitDefRef], PackageStoreError> {
        Ok(self
            .index()?
            .traits_by_const_name
            .get(name)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local traits that declare at least one function.
    pub(super) fn traits_with_functions(&self) -> Result<&[TraitDefRef], PackageStoreError> {
        Ok(self.index()?.traits_with_functions.as_slice())
    }

    /// Return body-local traits that declare at least one associated item.
    pub(super) fn traits_with_associated_items(&self) -> Result<&[TraitDefRef], PackageStoreError> {
        Ok(self.index()?.traits_with_associated_items.as_slice())
    }

    /// Build the request-local overlay once for queries throughout body inference.
    fn build_index(&self) -> Result<BodyLocalItemIndex, PackageStoreError> {
        let mut index = BodyLocalItemIndex::default();
        for store in self.body_lookup_stores()? {
            // Impl declarations provide receiver candidates. Inherent impls additionally retain
            // member names so an active declaration can replace its stale saved counterpart.
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() {
                    let trait_impl = TraitImplRef {
                        impl_ref,
                        trait_ref: *trait_ref,
                    };
                    index.trait_impls.push(trait_impl);
                    if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                        index
                            .trait_impls_by_type
                            .entry(*self_ty)
                            .or_default()
                            .push(trait_impl);
                    }
                    continue;
                }

                if impl_data.trait_ref.is_some() {
                    continue;
                }
                let Some(self_ty) = impl_data.resolved_self_ty.as_option() else {
                    continue;
                };
                index
                    .inherent_impls_by_type
                    .entry(*self_ty)
                    .or_default()
                    .push(impl_ref);
                let names = index
                    .inherent_item_names_by_type
                    .entry(*self_ty)
                    .or_default();
                for item in &impl_data.items {
                    match item {
                        AssocItemId::Function(id) => {
                            if let Some(data) = store.function_data(*id) {
                                names.functions.push(data.name.clone());
                            }
                        }
                        AssocItemId::Const(id) => {
                            if let Some(data) = store.const_data(*id) {
                                names.consts.push(data.name.clone());
                            }
                        }
                        AssocItemId::TypeAlias(id) => {
                            if let Some(data) = store.type_alias_data(*id) {
                                names.type_aliases.push(data.name.clone());
                            }
                        }
                    }
                }
            }

            // Trait declarations provide the name-first surfaces used before impl matching. A
            // method lookup for `value.draw()` discovers `Paint` here; receiver proof happens in
            // `BodyImplQuery` after lexical scope has filtered the declaration surface.
            for (trait_ref, trait_data) in store.traits_with_refs() {
                if !trait_data.items.is_empty() {
                    index.traits_with_associated_items.push(trait_ref);
                }

                let mut has_function = false;
                for item in &trait_data.items {
                    match item {
                        AssocItemId::Function(id) => {
                            has_function = true;
                            if let Some(data) = store.function_data(*id) {
                                index
                                    .traits_by_function_name
                                    .entry(data.name.clone())
                                    .or_default()
                                    .push(trait_ref);
                            }
                        }
                        AssocItemId::Const(id) => {
                            if let Some(data) = store.const_data(*id) {
                                index
                                    .traits_by_const_name
                                    .entry(data.name.clone())
                                    .or_default()
                                    .push(trait_ref);
                            }
                        }
                        AssocItemId::TypeAlias(_) => {}
                    }
                }
                if has_function {
                    index.traits_with_functions.push(trait_ref);
                }
            }
        }
        Ok(index)
    }

    fn index(&self) -> Result<&BodyLocalItemIndex, PackageStoreError> {
        self.context
            .body_local_item_cache()
            .index_or_try_init(|| self.build_index())
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
