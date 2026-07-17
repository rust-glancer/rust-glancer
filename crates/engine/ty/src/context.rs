//! Coherent inputs for crate-scoped type queries.

use rg_def_map::DefMapSource;
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreSource};

use crate::{ItemPathQuery, TraitSelectionSession};

/// Shared query environment for type reasoning at one crate use site.
///
/// Path lookup, visible-item lookup, the semantic lookup index, and trait selection all describe
/// one visibility universe. `TyContext` keeps that unit intact, constructs both item-query views
/// from the same routing providers, and prevents nested queries from silently replacing the shared
/// solver session.
///
/// For example, method lookup may autoderef a receiver and then prove a trait impl for the adjusted
/// type. Both steps must use the crate where the method is called as their use site; mixing a lookup
/// index from one crate with a solver session from another would produce a coherent-looking but
/// invalid result.
#[derive(Clone)]
pub struct TyContext<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    crate_items: CrateItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
    trait_selection: TraitSelectionSession,
}

impl<'query, D, I> TyContext<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Build one type-query environment using the solver session's crate as the use site.
    ///
    /// Deriving `CrateItemQuery` from the session is intentional: callers cannot pass a second,
    /// disagreeing use-site identity alongside it.
    pub fn new(
        def_maps: D,
        items: I,
        lookup_index: &'query ItemLookupIndex,
        trait_selection: TraitSelectionSession,
    ) -> Self {
        let use_site = trait_selection.use_site();
        Self {
            item_paths: ItemPathQuery::new(def_maps.clone(), items.clone()),
            crate_items: CrateItemQuery::new(def_maps, items, use_site),
            lookup_index,
            trait_selection,
        }
    }
}

impl<'query, D, I> TyContext<'query, D, I> {
    pub fn item_paths(&self) -> &ItemPathQuery<'query, D, I> {
        &self.item_paths
    }

    pub fn crate_items(&self) -> &CrateItemQuery<'query, D, I> {
        &self.crate_items
    }

    pub fn lookup_index(&self) -> &'query ItemLookupIndex {
        self.lookup_index
    }

    pub fn trait_selection(&self) -> &TraitSelectionSession {
        &self.trait_selection
    }
}
