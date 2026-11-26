//! Demand and validate saved query roots at the project boundary.

mod builder;
mod initial;
mod progress;

pub use self::builder::ProjectBuilder;
pub(crate) use self::initial::build_state;
