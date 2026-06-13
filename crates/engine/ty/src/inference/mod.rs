//! Temporary inference types and variables used before finalizing to `Ty`.
//!
//! This module keeps solver state separate from the persisted type vocabulary. Callers can carry
//! shapes such as `Vec<?T>` or `{integer}` during resolution, then finalize them back to `Ty`.

mod instantiate;
mod model;
mod table;

pub use instantiate::{
    ExplicitTypeArgInstantiationBuilder, GenericReturnInstantiationBuilder,
    UnknownTypeInstantiationBuilder,
};
pub use model::{InferGenericArg, InferNominalTy, InferOpaqueTraitBound, InferTy};
pub use table::{InferVarId, InferenceTable};

#[cfg(test)]
mod tests;
