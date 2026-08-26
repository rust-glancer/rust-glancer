//! Body-local item lookup for body-aware resolution.

use std::collections::HashSet;

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, FunctionRef, ImplRef, TraitDefRef, TraitImplRef,
    TypeAliasRef, TypeDefRef,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource, TraitData};
use rg_std::UniqueVec;
use rg_text::Name;

use crate::resolution::BodyResolutionContext;

/// Finds items declared in bodies, such as local impls and their methods.
pub(crate) struct BodyLocalItemQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// Names declared by body-local inherent impls, separated by associated-item kind.
#[derive(Default)]
pub(super) struct BodyLocalInherentItemNames {
    functions: UniqueVec<Name>,
    consts: UniqueVec<Name>,
    type_aliases: UniqueVec<Name>,
}

impl BodyLocalInherentItemNames {
    pub(super) fn extend(&mut self, other: Self) {
        self.functions.extend(other.functions);
        self.consts.extend(other.consts);
        self.type_aliases.extend(other.type_aliases);
    }

    pub(super) fn contains_function(&self, name: &str) -> bool {
        self.functions
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }

    pub(super) fn contains_const(&self, name: &str) -> bool {
        self.consts
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }

    pub(super) fn contains_type_alias(&self, name: &str) -> bool {
        self.type_aliases
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }
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

    /// Return names supplied by body-local inherent impls for this type.
    ///
    /// Body-aware lookup combines these impls with a crate-wide index. A current declaration
    /// replaces a crate-indexed declaration of the same kind and name; unrelated saved members
    /// remain visible.
    pub(super) fn inherent_item_names_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<BodyLocalInherentItemNames, PackageStoreError> {
        let item_query = self.context.item_query();
        let mut functions = UniqueVec::new();
        let mut consts = UniqueVec::new();
        let mut type_aliases = UniqueVec::new();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            for item in &impl_data.items {
                match item {
                    AssocItemId::Function(id) => {
                        let function = FunctionRef {
                            origin: impl_ref.origin,
                            id: *id,
                        };
                        if let Some(data) = item_query.function_data(function)? {
                            functions.push(data.name.clone());
                        }
                    }
                    AssocItemId::Const(id) => {
                        let konst = ConstRef {
                            origin: impl_ref.origin,
                            id: *id,
                        };
                        if let Some(data) = item_query.const_data(konst)? {
                            consts.push(data.name.clone());
                        }
                    }
                    AssocItemId::TypeAlias(id) => {
                        let alias = TypeAliasRef {
                            origin: impl_ref.origin,
                            id: *id,
                        };
                        if let Some(data) = item_query.type_alias_data(alias)? {
                            type_aliases.push(data.name.clone());
                        }
                    }
                }
            }
        }
        Ok(BodyLocalInherentItemNames {
            functions,
            consts,
            type_aliases,
        })
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

    /// Return body-local impls for already-selected traits in one store pass.
    pub(super) fn trait_impls_for_traits(
        &self,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let trait_refs = trait_refs.into_iter().collect::<HashSet<_>>();
        if trait_refs.is_empty() {
            return Ok(UniqueVec::new());
        }

        let mut trait_impls = UniqueVec::new();
        for store in self.body_lookup_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() else {
                    continue;
                };
                if !trait_refs.contains(trait_ref) {
                    continue;
                }
                trait_impls.push(TraitImplRef {
                    impl_ref,
                    trait_ref: *trait_ref,
                });
            }
        }

        Ok(trait_impls)
    }

    /// Return body-local traits declaring a function with this name.
    pub(super) fn traits_with_function_name(
        &self,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        self.trait_refs_matching(|store, trait_data| {
            trait_data.items.iter().any(|item| {
                let AssocItemId::Function(id) = item else {
                    return false;
                };
                store
                    .function_data(*id)
                    .is_some_and(|data| data.name == name)
            })
        })
    }

    /// Return body-local traits declaring an associated const with this name.
    pub(super) fn traits_with_const_name(
        &self,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        self.trait_refs_matching(|store, trait_data| {
            trait_data.items.iter().any(|item| {
                let AssocItemId::Const(id) = item else {
                    return false;
                };
                store.const_data(*id).is_some_and(|data| data.name == name)
            })
        })
    }

    /// Return body-local traits that declare at least one function.
    pub(super) fn traits_with_functions(
        &self,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        self.trait_refs_matching(|_, trait_data| {
            trait_data
                .items
                .iter()
                .any(|item| matches!(item, AssocItemId::Function(_)))
        })
    }

    /// Return body-local traits that declare at least one associated item.
    pub(super) fn traits_with_associated_items(
        &self,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        self.trait_refs_matching(|_, trait_data| !trait_data.items.is_empty())
    }

    /// Filter body-local trait declarations without rebuilding the body-store routing in callers.
    fn trait_refs_matching(
        &self,
        predicate: impl Fn(&ItemStore, &TraitData) -> bool,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        let mut traits = UniqueVec::new();
        for store in self.body_lookup_stores()? {
            for (trait_ref, trait_data) in store.traits_with_refs() {
                if predicate(store, trait_data) {
                    traits.push(trait_ref);
                }
            }
        }
        Ok(traits)
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
