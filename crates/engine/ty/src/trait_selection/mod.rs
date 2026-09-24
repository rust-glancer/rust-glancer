//! Owned trait-selection and normalization queries over the compiler solver.
//!
//! Each query lowers durable types into a temporary inference table and freezes the result before
//! returning. Live body inference uses `solver` directly so its variables survive across operations.

mod goal;
mod query;
pub use self::{
    goal::TraitGoal,
    query::{AssocProjectionResult, TraitSelection, TraitSelectionQuery},
};
#[cfg(test)]
mod tests;
