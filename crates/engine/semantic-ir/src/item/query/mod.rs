//! Shared queries over semantic-shaped item stores.
//!
//! Crate and body IR store item data in the same `ItemStore` shape. This module separates raw
//! store routing from queries that need a concrete Rust visibility universe.

mod crate_item;
mod generics;
mod item_store;
mod lookup;
mod resolution;

use rg_ir_model::DefMapRef;

pub use self::{
    crate_item::CrateItemQuery,
    generics::GenericsQuery,
    item_store::ItemStoreQuery,
    lookup::{ItemLookupQuery, ItemLookupQueryCache, ItemLookupQueryCacheStats},
    resolution::ItemResolutionQuery,
};
use crate::ItemStore;

/// Provides the stores that semantic-shaped item refs can point into.
///
/// Layer-specific code implements this once, and the query modules can then treat crate items and
/// body-local items as the same kind of data.
pub trait ItemStoreSource<'a>: Clone {
    type Error;

    /// Finds the store that owns refs with this origin.
    ///
    /// `None` means the origin is outside of the source's view, for example a different body.
    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error>;
}
