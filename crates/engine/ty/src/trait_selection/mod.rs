//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! Native matching discovers canonical impl headers that may fit a resolved trait goal. A small
//! bounded native proof handles concrete impl chains and compiler-known closure facts; Chalk owns
//! the remaining predicates and associated-type equalities. Keeping discovery, native proof, and
//! solver fallback as different types prevents exploratory editor candidates from being mistaken
//! for established semantic facts.
//!
//! Canonical crate declarations have a wider reuse boundary than solver state. Their lowered types
//! can be shared by sessions over the same semantic snapshot, while visible impl indexes, Chalk
//! forests, body declarations, and inference answers remain owned by the use-site session that
//! produced them.

mod candidate;
mod chalk;
mod declaration_cache;
mod goal;
mod matcher;
mod native_proof;
mod projection;
mod query;
mod session;
mod work;

pub use declaration_cache::TraitSelectionDeclarationCache;
pub use goal::TraitGoal;
pub use projection::AssocProjectionResult;
pub use query::{TraitProof, TraitSelection, TraitSelectionQuery};
pub(crate) use session::CachedImplSelfMatch;
pub use session::TraitSelectionSession;

#[cfg(test)]
mod tests;
