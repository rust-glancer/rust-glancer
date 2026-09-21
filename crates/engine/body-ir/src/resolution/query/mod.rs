//! Body-scoped lookup and signature queries.
//!
//! Each query combines lexical body scopes with the declarations and types supplied by its
//! resolution context. Candidate and result types stay beside the operations that produce them.

mod associated_item;
mod body_items;
mod call;
mod callable;
mod field;
mod function;
mod generics;
mod impls;
mod method;
mod traits;
mod type_context;
mod type_path;
mod type_ref;
mod value_path;

pub(crate) use self::{
    associated_item::BodyAssociatedItemQuery,
    body_items::BodyLocalItemQuery,
    call::{BodyCallQuery, CallSelfSource},
    callable::BodyCallableCandidate,
    field::BodyFieldQuery,
    function::BodyFunctionQuery,
    generics::BodyGenericsQuery,
    impls::{BodyImplQuery, BodyReceiverImplMatches},
    traits::BodyTraitQuery,
    type_context::BodyTypeContextQuery,
    type_ref::TypeRefResolutionQuery,
};
pub use self::{
    method::BodyMethodQuery, type_path::BodyTypePathQuery, value_path::BodyValuePathQuery,
};
