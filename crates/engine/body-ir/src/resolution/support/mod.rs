//! Shared resolution helpers that are used by more than one resolution stage.
//!
//! This module is not a generic utils junkyard. Helpers here should model a
//! resolution concept that does not belong to only one pass/query/inference step.

mod callable;
mod impl_predicate;
mod impl_predicate_assoc;
mod selected_trait_assoc;
mod ty_normalize;
mod type_ref_projector;

pub(crate) use self::callable::{
    CallableTypeRefExpectation, CallableTypeResolver, callable_arg_expectations,
};
pub(crate) use self::impl_predicate::{ImplPredicateSubject, impl_projection_predicates};
pub(crate) use self::impl_predicate_assoc::{
    ImplPredicateAssocProjection, ImplPredicateAssocProjector, ProjectionSupport,
    project_unique_support_assoc,
};
pub(crate) use self::selected_trait_assoc::{
    SelectedTraitMethodContext, self_associated_type_name,
};
pub(crate) use self::ty_normalize::TyNormalizer;
pub(crate) use self::type_ref_projector::BodyTypeRefProjector;
