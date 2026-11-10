//! Body data and read access for saved and current source.

mod body_local_items;
mod body_store;
pub(crate) mod current;
pub(crate) mod reader;

pub(crate) use self::current::CurrentBody;
pub use self::{
    body_local_items::BodyLocalItems, body_store::BodyStore, current::CurrentSourceStore,
    reader::BodyReader,
};
