//! Body-local inference facts used before writing resolved `Ty` values.
//!
//! The persisted Body IR model stores ordinary `Ty` facts. This module keeps inference variables
//! in a transient table so later resolver phases can preserve relationships such as `Vec<?T>`
//! until local evidence solves `?T`.

mod context;
mod instantiate;
mod model;
mod table;

pub(super) use context::BodyInferenceCtx;

#[cfg(test)]
mod tests;
