//! Body-local inference facts used before writing resolved `Ty` values.
//!
//! The persisted Body IR model stores ordinary `Ty` facts. This module only maps body expression
//! and binding slots to the transient inference table owned by `rg_ty`.

mod call;
mod context;
mod facts;
mod member;
mod pattern;
mod trait_obligation;

pub(super) use call::BodyCallInference;
pub(super) use context::BodyInferenceCtx;
pub(super) use member::BodyMemberInference;
pub(super) use pattern::BodyPatternInference;

#[cfg(test)]
mod tests;
