//! Chalk-backed trait solving for `TraitSelectionQuery`.
//!
//! The public query still returns project-native `TraitSelection` values. Chalk is used as the
//! real trait solver behind that facade, while the surrounding code keeps owning visibility,
//! source-path resolution, and inference-table commits.

mod interner;
mod lower;
mod program;
mod projection;
mod raise;

pub(super) use self::program::ChalkTraitSolver;
