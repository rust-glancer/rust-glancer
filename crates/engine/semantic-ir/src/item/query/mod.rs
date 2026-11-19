//! Shared semantic item queries over crate and body-local declarations.

mod candidates;
mod generics;
mod item_store;
mod lookup;
mod resolution;

use rg_ir_model::{CrateRef, DefMapRef, ItemStoreRef, LocalDefRef, SemanticItemRef};
use rg_source::ReadError;

pub use self::{
    candidates::{LookupAnswer, LookupQuestion},
    generics::GenericsQuery,
    item_store::ItemStoreQuery,
    lookup::ItemLookupQuery,
    resolution::ItemResolutionQuery,
};
use crate::{AssociatedItemStore, SemanticItemView, SourceSelection};

/// Supplies borrowed declarations without requiring a complete store for their origin.
///
/// Providers acquire the data containing an item before selecting it. Missing items and local
/// definitions must observe that data too, so a later update can invalidate an absent result.
pub trait SemanticItemSource<'a>: Clone {
    /// Open lookup from the declaration's crate, even when there is no caller's solver.
    /// Sources that only hold local declarations may have no crate lookup.
    fn item_lookup_for_crate(
        &self,
        _krate: CrateRef,
    ) -> Result<Option<ItemLookupQuery<'a>>, ReadError> {
        Ok(None)
    }

    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError>;

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError>;

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError>;

    /// Read declarations in the lexical scope, including trait and impl headers, without
    /// acquiring their associated stores. Callers can select an owner before reading its members.
    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError>;

    /// Enumerate all declarations in the origin, acquiring every associated store as well.
    /// For a crate origin this can expand every member macro; selecting only an impl header should
    /// use `scope_items` instead.
    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError>;

    /// Select declaration candidates using an explicit source surface. Written cursor/outline
    /// selection can stay local; exhaustive file scans also discover owners that reuse the file
    /// through macros. The caller checks exact ranges.
    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError>;
}
