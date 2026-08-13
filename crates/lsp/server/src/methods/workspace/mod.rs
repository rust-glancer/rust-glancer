//! Project-wide handlers that use saved analysis from the active engine.
//!
//! These methods do not target an open editor document, so they take `EngineClient` directly
//! instead of carrying a document or completion context.

pub(crate) mod execute_command;
pub(crate) mod symbol;
