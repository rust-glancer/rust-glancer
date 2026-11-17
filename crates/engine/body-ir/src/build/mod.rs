//! Lower and infer bodies from the active input view.

mod local_items;
mod lower;
mod pattern_binding;
mod queries;
mod query_source;
mod state;

pub use self::queries::{PreparedBody, body_store, infer_body, prepare_body};
