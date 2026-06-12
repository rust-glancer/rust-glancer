//! Body-local inference facts used before writing resolved `Ty` values.
//!
//! The persisted Body IR model stores ordinary `Ty` facts. This module only maps body expression
//! and binding slots to the transient inference table owned by `rg_ty`.

mod context;

pub(super) use context::BodyInferenceCtx;

#[cfg(test)]
mod tests;
