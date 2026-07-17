//! Chalk-backed trait solving for `TraitSelectionQuery`.
//!
//! The public query still returns project-native `TraitSelection` values. Chalk is used as the
//! real trait solver behind that facade, while the surrounding code keeps owning visibility,
//! source-path resolution, and inference-table commits.
//!
//! `lower` and `raise` translate at the solver boundary. `program` builds the goal-reachable Rust
//! program Chalk reads, and `solver` owns the mutable forests that answer queries against it.

mod interner;
mod lower;
mod program;
mod projection;
mod raise;
mod solver;

pub(super) use self::solver::{ChalkOutcome, ChalkTraitSolver};
