//! Shared semantic item queries over crate and body-local declarations.

mod generics;
mod item_store;
mod lookup;
mod resolution;

use rg_ir_model::{DefMapRef, LocalDefRef, SemanticItemRef};

pub use self::{
    generics::GenericsQuery,
    item_store::ItemStoreQuery,
    lookup::{ItemLookupQuery, ItemLookupQueryCache, ItemLookupQueryCacheStats},
    resolution::ItemResolutionQuery,
};
use crate::SemanticItemView;

/// Supplies borrowed declarations without requiring a complete store for their origin.
///
/// Providers acquire the data containing an item before selecting it. Missing items and local
/// definitions must observe that data too, so a later update can invalidate an absent result.
pub trait SemanticItemSource<'a>: Clone {
    type Error;

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, Self::Error>;

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, Self::Error>;

    /// Acquire each containing group once and enumerate its items in storage order.
    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, Self::Error>;
}
