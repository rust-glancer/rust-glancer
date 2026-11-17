//! Shared semantic item queries over crate and body-local declarations.

mod candidates;
mod generics;
mod item_store;
mod lookup;
mod resolution;

use rg_ir_model::{DefMapRef, ItemStoreRef, LocalDefRef, SemanticItemRef};
use rg_source::ReadError;

pub use self::{
    candidates::{LookupAnswer, LookupQuestion},
    generics::GenericsQuery,
    item_store::ItemStoreQuery,
    lookup::ItemLookupQuery,
    resolution::ItemResolutionQuery,
};
use crate::{AssociatedItemStore, SemanticItemView};

/// Supplies borrowed declarations without requiring a complete store for their origin.
///
/// Providers acquire the data containing an item before selecting it. Missing items and local
/// definitions must observe that data too, so a later update can invalidate an absent result.
pub trait SemanticItemSource<'a>: Clone {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError>;

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError>;

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError>;

    /// Acquire each containing group once and enumerate its items in storage order.
    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError>;
}
