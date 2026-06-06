//! Target-scoped item lookup.

use rg_ir_model::{
    DefMapRef, FunctionRef, ImplRef, ModuleRef, TargetRef, TraitImplRef, TraitRef, TypeDefRef,
};

use super::{ItemStoreQuery, ItemStoreSource};
use crate::{DefMapQuery, DefMapSource, ItemStore, TargetResolutionEnv, push_unique};

/// Item queries that need a Rust language visibility context.
///
/// Raw item refs can be read directly from `ItemStoreQuery`. Impl and method lookup are different:
/// they need the set of item stores visible from the target where lookup happens.
#[derive(Clone)]
pub struct TargetItemQuery<'item, D, I> {
    def_maps: DefMapQuery<D>,
    items: ItemStoreQuery<'item, I>,
    use_site: TargetRef,
}

impl<'item, D, I> TargetItemQuery<'item, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'item>,
{
    pub fn new(def_maps: D, items: I, use_site: TargetRef) -> Self {
        Self {
            def_maps: DefMapQuery::new(def_maps),
            items: ItemStoreQuery::new(items),
            use_site,
        }
    }

    pub fn items(&self) -> &ItemStoreQuery<'item, I> {
        &self.items
    }

    /// Returns the root module of the target where lookup is performed.
    pub fn use_site_root_module(&self) -> Result<Option<ModuleRef>, I::Error> {
        self.def_maps.root_module(self.use_site)
    }

    /// Returns stores visible from this query's use-site target.
    pub fn visible_stores(&self) -> Result<Vec<&'item ItemStore>, I::Error> {
        let targets = self.def_maps.visible_targets_from(self.use_site)?;
        self.items.stores_for_targets(&targets)
    }

    /// Searches impls visible from the use-site target, not from the receiver type's origin.
    pub fn impls_for_type(&self, ty: TypeDefRef) -> Result<Vec<ImplRef>, I::Error> {
        let mut impls = Vec::new();
        for store in self.impl_stores_for_origin(ty.origin)? {
            impls.extend(store.impls_with_refs().filter_map(|(impl_ref, data)| {
                data.resolved_self_tys.contains(&ty).then_some(impl_ref)
            }));
        }
        Ok(impls)
    }

    /// Searches visible impls for a trait ref while keeping duplicate refs out of the result.
    pub fn impls_for_trait(&self, trait_ref: TraitRef) -> Result<Vec<ImplRef>, I::Error> {
        let mut impls = Vec::new();
        for store in self.impl_stores_for_origin(trait_ref.origin)? {
            for (impl_ref, data) in store.impls_with_refs() {
                if data.resolved_trait_refs.contains(&trait_ref) {
                    push_unique(&mut impls, impl_ref);
                }
            }
        }
        Ok(impls)
    }

    /// Narrows type impl lookup to inherent impls, which is the path used for method completion.
    pub fn inherent_impls_for_type(&self, ty: TypeDefRef) -> Result<Vec<ImplRef>, I::Error> {
        let mut impls = Vec::new();
        for impl_ref in self.impls_for_type(ty)? {
            let Some(data) = self.items.impl_data(impl_ref)? else {
                continue;
            };
            if data.trait_ref.is_none() {
                impls.push(impl_ref);
            }
        }
        Ok(impls)
    }

    /// Searches all visible inherent impls, including impls whose `Self` type is structural.
    pub fn inherent_impls(&self) -> Result<Vec<ImplRef>, I::Error> {
        let mut impls = Vec::new();
        for store in self.visible_stores()? {
            for (impl_ref, data) in store.impls_with_refs() {
                if data.trait_ref.is_none() {
                    push_unique(&mut impls, impl_ref);
                }
            }
        }
        Ok(impls)
    }

    /// Collects inherent functions for callers that care about callable members, not impl blocks.
    pub fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, I::Error> {
        let mut functions = Vec::new();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(data) = self.items.impl_data(impl_ref)? else {
                continue;
            };
            functions.extend(data.functions());
        }
        Ok(functions)
    }

    /// Expands matching trait impl blocks into the trait refs they actually implement.
    pub fn trait_impls_for_type(&self, ty: TypeDefRef) -> Result<Vec<TraitImplRef>, I::Error> {
        let mut trait_impls = Vec::new();
        for impl_ref in self.impls_for_type(ty)? {
            let Some(data) = self.items.impl_data(impl_ref)? else {
                continue;
            };

            for trait_ref in &data.resolved_trait_refs {
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

    /// Lists trait declarations implemented by the visible impls for a nominal type.
    pub fn traits_for_type(&self, ty: TypeDefRef) -> Result<Vec<TraitRef>, I::Error> {
        let mut traits = Vec::new();
        for trait_impl in self.trait_impls_for_type(ty)? {
            push_unique(&mut traits, trait_impl.trait_ref);
        }
        Ok(traits)
    }

    /// Collects trait-declared functions available for a nominal type.
    pub fn trait_functions_for_type(&self, ty: TypeDefRef) -> Result<Vec<FunctionRef>, I::Error> {
        let mut functions = Vec::new();
        for trait_ref in self.traits_for_type(ty)? {
            let Some(data) = self.items.trait_data(trait_ref)? else {
                continue;
            };
            for function in data.functions() {
                push_unique(&mut functions, function);
            }
        }
        Ok(functions)
    }

    /// Collects concrete trait-impl functions available for a nominal type.
    pub fn trait_impl_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, I::Error> {
        let mut functions = Vec::new();
        for trait_impl in self.trait_impls_for_type(ty)? {
            let Some(data) = self.items.impl_data(trait_impl.impl_ref)? else {
                continue;
            };
            functions.extend(data.functions());
        }
        Ok(functions)
    }

    /// Target-origin impl lookup sees the use-site target's visible semantic stores; body-local refs
    /// stay scoped to their owning body store.
    fn impl_stores_for_origin(&self, origin: DefMapRef) -> Result<Vec<&'item ItemStore>, I::Error> {
        if origin.as_target_ref().is_some() {
            return self.visible_stores();
        }

        Ok(self
            .items
            .item_store_for_origin(origin)?
            .into_iter()
            .collect::<Vec<_>>())
    }
}
