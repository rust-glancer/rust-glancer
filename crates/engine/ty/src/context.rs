//! Coherent inputs for crate-scoped type queries.

use rg_def_map::DefMapSource;
use rg_ir_model::CrateRef;
use rg_semantic_ir::{CrateItemQuery, ItemLookupQuery, ItemStoreSource};
use rg_std::CancellationToken;

use crate::lookup::ItemPathQuery;

/// Shared query environment for type reasoning at one crate use site.
///
/// Path lookup, visible-item lookup, the semantic lookup query, and trait selection all describe
/// one visibility universe. `TyContext` keeps that unit intact and carries cancellation through
/// nested queries. Solver operations borrow this context and own their inference state separately.
///
/// For example, method lookup may autoderef a receiver and then prove a trait impl for the adjusted
/// type. Both steps must use the crate where the method is called as their use site.
#[derive(Clone)]
pub struct TyContext<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    crate_items: CrateItemQuery<'query, D, I>,
    item_lookup: ItemLookupQuery<'query>,
    cancellation: CancellationToken,
}

impl<D, I> rg_std::Cancelable for TyContext<'_, D, I> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'query, D, I> TyContext<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub fn new(
        def_maps: D,
        items: I,
        item_lookup: ItemLookupQuery<'query>,
        use_site: CrateRef,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps.clone(), items.clone()),
            crate_items: CrateItemQuery::new(def_maps, items, use_site),
            item_lookup,
            cancellation,
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

    pub fn item_lookup(&self) -> &ItemLookupQuery<'query> {
        &self.item_lookup
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}
