//! Shared resolution helpers that are used by more than one resolution stage.
//!
//! This module is not a generic utils junkyard. Helpers here should model a
//! resolution concept that does not belong to only one pass/query/inference step.

mod callable;
mod selected_trait_assoc;
mod ty_normalize;

pub(crate) use self::callable::{
    CallableTypeRefExpectation, CallableTypeResolver, callable_arg_expectations,
};
pub(crate) use self::selected_trait_assoc::{
    SelectedTraitAssocProjector, SelectedTraitMethodContext, self_associated_type_name,
};
pub(crate) use self::ty_normalize::TyNormalizer;
