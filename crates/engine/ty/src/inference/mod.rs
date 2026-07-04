//! Temporary inference state used before finalizing `Ty`.
//!
//! Inference variables are ordinary `Ty` nodes, but they must not cross the finalization boundary.
//! The table owns those variables, solves them, and replaces any leftover transient state before
//! types become persistent compiler facts.

mod instantiate;
mod subst;
mod table;
mod traversal;
mod var;

pub use instantiate::{
    ExplicitTypeArgInstantiationBuilder, GenericReturnInstantiationBuilder,
    UnknownTypeInstantiationBuilder,
};
pub use subst::{InferenceTypeRefProjector, InferenceTypeSubst};
pub use table::{InferenceConflict, InferenceTable};
pub use var::{InferVarId, InferVarKind};

#[cfg(test)]
mod tests;
