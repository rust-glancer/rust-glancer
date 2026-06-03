//! Shared queries over semantic-shaped item stores.
//!
//! Target and body IR store item data in the same `ItemStore` shape. This module separates raw
//! store routing from queries that need a concrete Rust visibility universe.

mod item_store;
mod target;

use rg_ir_model::DefMapRef;

use crate::ItemStore;

pub use self::{item_store::ItemStoreQuery, target::TargetItemQuery};

/// Provides the stores that semantic-shaped item refs can point into.
///
/// Layer-specific code implements this once, and the query modules can then treat target items and
/// body-local items as the same kind of data.
pub trait ItemStoreSource<'a> {
    type Error;

    /// Finds the store that owns refs with this origin.
    ///
    /// `None` means the origin is outside of the source's view, for example a different body.
    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error>;

    /// Enumerates all stores materialized by the source.
    ///
    /// This is a storage boundary, not a language visibility boundary. Impl and method lookup use
    /// `TargetItemQuery`, which derives visibility from a concrete use-site target through DefMap
    /// data.
    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error>;
}
