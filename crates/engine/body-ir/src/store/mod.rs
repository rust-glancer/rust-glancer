//! Body data and on-demand read access.

mod body_local_items;
mod body_store;
mod prepared;
pub(crate) mod reader;

pub use self::{
    body_local_items::BodyLocalItems,
    body_store::BodyStore,
    prepared::{BodyFactsStore, PreparedBodyStore},
    reader::{BodyReader, DeclarationSourceView},
};
