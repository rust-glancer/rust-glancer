//! Crate-scoped item lookup.

use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::CrateRef;

use super::{ItemLookupIndexSource, ItemStoreQuery, ItemStoreSource};
use crate::{ItemLookupIndex, ItemStore};

/// Item queries that need a Rust language visibility context.
///
/// Raw item refs can be read directly from `ItemStoreQuery`. Lookup-index construction and trait solver
/// declaration discovery instead need the set of item stores visible from the crate where lookup
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
        self.items.stores_for_crates(crates.as_slice())
    }

    /// Returns visible crate identities paired with declaration-local lookup indexes.
    ///
    /// Crate identities stay beside the indexes so language-item entries can recover their complete
    /// semantic origins without loading the corresponding declaration stores.
    pub(super) fn visible_indexes(
        &self,
        cancellation: &rg_std::CancellationToken,
    ) -> Result<Vec<(CrateRef, &'item ItemLookupIndex)>, rg_std::OperationError<I::Error>>
    where
        I: ItemLookupIndexSource<'item>,
    {
        let crates = self
            .def_maps
            .item_lookup_crates_from(self.use_site)
            .map_err(rg_std::OperationError::Source)?;
        self.items
            .indexes_for_crates(crates.as_slice(), cancellation)
    }
}
