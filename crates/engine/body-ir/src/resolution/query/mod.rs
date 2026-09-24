//! Body-scoped lookup and signature queries.
//!
//! Each query combines lexical body scopes with the declarations and types supplied by its
//! resolution context. Candidate and result types stay beside the operations that produce them.
//! Queries over owned types serve source lowering and editor requests. `live` keeps the same
//! lookup context while working with inference variables that can still gain evidence in a body.

mod associated_item;
mod body_items;
mod callable;
mod field;
mod function;
mod generics;
mod impls;
mod live;
mod method;
mod traits;
mod type_context;
mod type_path;
mod type_ref;
mod value_path;

pub(crate) use self::{
    associated_item::BodyAssociatedItemQuery,
    body_items::BodyLocalItemQuery,
    callable::BodyCallableCandidate,
    field::BodyFieldQuery,
    function::BodyFunctionQuery,
    generics::BodyGenericsQuery,
    impls::{BodyImplQuery, BodyReceiverImplMatches},
    live::LiveBodyQuery,
    traits::BodyTraitQuery,
    type_context::BodyTypeContextQuery,
    type_ref::TypeRefResolutionQuery,
};
pub use self::{
    method::BodyMethodQuery, type_path::BodyTypePathQuery, value_path::BodyValuePathQuery,
};
