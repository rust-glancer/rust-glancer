//! Navigation-related editor query implementations.
//!
//! This module separates the navigation pipeline into small layers: target projection turns known
//! IR IDs into public navigation payloads, symbol resolution turns cursor symbols into those IDs,
//! and the public goto query flows compose symbol/type lookup with target projection.

mod goto;
mod implementation;
mod symbol;
mod target;
mod type_definition;

pub(crate) use self::{
    goto::GotoResolver, implementation::ImplementationResolver, symbol::SymbolResolver,
    type_definition::TypeDefinitionResolver,
};
