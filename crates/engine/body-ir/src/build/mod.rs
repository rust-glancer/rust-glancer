//! Lower bodies, collect their local declarations, and infer expression types and values.

mod local_items;
mod lower;
mod pattern_binding;
mod queries;
mod query_source;
mod state;

pub use self::queries::{PreparedBody, body_store, infer_body, prepare_body};
