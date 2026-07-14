//! Shared resolution helpers that are used by more than one resolution stage.
//!
//! This module is not a generic utils junkyard. Helpers here should model a
//! resolution concept that does not belong to only one pass/query/inference step.

mod body_assoc_projector;
mod callable;
mod ty_normalize;

pub(crate) use self::body_assoc_projector::BodyAssocProjector;
pub(crate) use self::callable::callable_arg_expectations;
pub(crate) use self::ty_normalize::TyNormalizer;
