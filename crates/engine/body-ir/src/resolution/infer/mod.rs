//! Body-local inference state used before writing resolved `Ty` values.
//!
//! One context owns expression and binding slots, selected-call substitutions, and body-local
//! obligation evidence. It is consumed at the persistence boundary, where every transient
//! inference variable is finalized into `BodyFacts`.

mod call;
mod context;
mod facts;
mod member;
mod pattern;
mod trait_obligation;

pub(super) use call::BodyCallInference;
pub(super) use context::{BodyInferenceCtx, BodyInferenceSnapshot};
pub(super) use member::BodyMemberInference;
pub(super) use pattern::BodyPatternInference;

#[cfg(test)]
mod tests;
