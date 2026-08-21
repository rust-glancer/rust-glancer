//! Crate-scoped item lookup.

use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::{CrateRef, DefMapRef, ImplRef, TraitDefRef};
use rg_std::UniqueVec;

use super::{ItemLookupIndexSource, ItemStoreQuery, ItemStoreSource};
use crate::{ItemLookupIndex, ItemStore};

/// Item queries that need a Rust language visibility context.
///
/// Raw item refs can be read directly from `ItemStoreQuery`. Lookup-index construction and Chalk
/// program discovery instead need the set of item stores visible from the crate where lookup
/// happens.
#[derive(Clone)]
pub struct CrateItemQuery<'item, D, I> {
    def_maps: DefMapQuery<D>,
    items: ItemStoreQuery<'item, I>,
    use_site: CrateRef,
}

impl<'item, D, I> CrateItemQuery<'item, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'item>,
{
    pub fn new(def_maps: D, items: I, use_site: CrateRef) -> Self {
        Self {
            def_maps: DefMapQuery::new(def_maps),
            items: ItemStoreQuery::new(items),
            use_site,
        }
    }

    pub fn items(&self) -> &ItemStoreQuery<'item, I> {
        &self.items
    }

    pub fn use_site(&self) -> CrateRef {
        self.use_site
    }

    /// Returns ordinary semantic stores participating in lookup from the use-site crate.
    ///
    /// Macro resolution has its own namespace reachability. Proc-macro implementation stores do
    /// not enter this item universe when the macro crate is an external dependency.
    pub fn visible_stores(&self) -> Result<Vec<&'item ItemStore>, I::Error> {
        let crates = self.def_maps.item_lookup_crates_from(self.use_site)?;
        self.items.stores_for_crates(&crates)
    }

    /// Returns the visible semantic stores paired with declaration-local lookup indexes.
    pub fn visible_indexed_stores(
        &self,
    ) -> Result<Vec<(&'item ItemStore, &'item ItemLookupIndex)>, I::Error>
    where
        I: ItemLookupIndexSource<'item>,
    {
        let crates = self.def_maps.item_lookup_crates_from(self.use_site)?;
        self.items.indexed_stores_for_crates(&crates)
    }

    /// Searches visible impls for a trait ref while keeping duplicate refs out of the result.
    pub fn impls_for_trait(&self, trait_ref: TraitDefRef) -> Result<UniqueVec<ImplRef>, I::Error> {
        let mut impls = UniqueVec::new();
        for store in self.impl_stores_for_origin(trait_ref.origin)? {
            for (impl_ref, data) in store.impls_with_refs() {
                if data.resolved_trait_ref.is(&trait_ref) {
                    impls.push(impl_ref);
                }
            }
        }
        Ok(impls)
    }

    /// Crate-origin impl lookup sees the use-site crate's visible semantic stores; body-local refs
    /// stay scoped to their owning body store.
    fn impl_stores_for_origin(&self, origin: DefMapRef) -> Result<Vec<&'item ItemStore>, I::Error> {
        if origin.as_crate_ref().is_some() {
            return self.visible_stores();
        }

        Ok(self
            .items
            .item_store_for_origin(origin)?
            .into_iter()
            .collect::<Vec<_>>())
    }
}
