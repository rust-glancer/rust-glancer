//! Small adapter from Rust's generic syntax to the predicate shape body resolution needs.
//!
//! The same semantic predicate can be written either inline:
//!
//! ```text
//! impl<S: Source> Produces for Adapter<S>
//! ```
//!
//! or in a where-clause:
//!
//! ```text
//! impl<S> Produces for Adapter<S>
//! where
//!     S: Source,
//! ```
//!
//! Body projection should not care which spelling the library author picked, so this module
//! normalizes both into `subject + bounds` entries.

use rg_ir_model::items::{GenericParams, TypeBound, TypeRef, WherePredicate};
use rg_text::Name;

/// One impl predicate after erasing the difference between inline and where-clause spelling.
///
/// The important part is that `impl<S: Source>` should reach projection as the same kind of
/// evidence as `where S: Source`, without pretending that the inline `S` was written as a
/// standalone `TypeRef`.
pub(crate) struct ImplProjectionPredicate<'a> {
    pub(crate) subject: ImplPredicateSubject,
    pub(crate) bounds: &'a [TypeBound],
}

/// The left-hand side of an impl predicate.
///
/// Inline generic bounds name a declaration parameter directly, while where-clauses keep the
/// written type syntax. Keeping those cases separate matters because `TypeRef` means "syntax from
/// the source file"; callers should not need fake syntax to handle `impl<S: Source>`.
pub(crate) enum ImplPredicateSubject {
    /// Inline generic bounds such as `impl<S: Source>`.
    TypeParam(Name),
    /// Explicit where-predicate subjects such as `where Wrapper<S>: Source`.
    TypeRef(TypeRef),
}

impl ImplPredicateSubject {
    pub(crate) fn type_param_name(&self) -> Option<Name> {
        match self {
            Self::TypeParam(name) => Some(name.clone()),
            Self::TypeRef(ty) => ty.type_param_name(),
        }
    }
}

impl ImplProjectionPredicate<'_> {
    fn explicit<'a>(ty: &TypeRef, bounds: &'a [TypeBound]) -> ImplProjectionPredicate<'a> {
        ImplProjectionPredicate {
            subject: ImplPredicateSubject::TypeRef(ty.clone()),
            bounds,
        }
    }

    fn type_param<'a>(name: &Name, bounds: &'a [TypeBound]) -> ImplProjectionPredicate<'a> {
        ImplProjectionPredicate {
            subject: ImplPredicateSubject::TypeParam(name.clone()),
            bounds,
        }
    }
}

/// Return the impl predicates that body-local projection knows how to reason about.
///
/// Unsupported predicate families make the whole stream unavailable. That keeps callers
/// conservative: if an impl has lifetime predicates or non-type where predicates we do not model,
/// associated projection should stay unknown instead of silently ignoring those obligations.
pub(crate) fn impl_projection_predicates(
    generics: &GenericParams,
) -> Option<Vec<ImplProjectionPredicate<'_>>> {
    if generics
        .lifetimes
        .iter()
        .any(|param| !param.bounds.is_empty())
    {
        return None;
    }

    let mut predicates = Vec::new();
    for param in &generics.types {
        if param.bounds.is_empty() {
            continue;
        }
        predicates.push(ImplProjectionPredicate::type_param(
            &param.name,
            &param.bounds,
        ));
    }

    for predicate in &generics.where_predicates {
        let WherePredicate::Type { ty, bounds } = predicate else {
            return None;
        };
        predicates.push(ImplProjectionPredicate::explicit(ty, bounds));
    }

    Some(predicates)
}
