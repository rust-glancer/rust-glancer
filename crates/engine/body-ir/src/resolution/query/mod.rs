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
mod type_alias;
mod type_context;
mod type_path;
mod type_ref;
mod value_path;

pub use self::{
    method::BodyMethodQuery, type_path::BodyTypePathQuery, value_path::BodyValuePathQuery,
};

pub(crate) use self::{
    associated_item::BodyAssociatedItemQuery,
    body_items::BodyLocalItemQuery,
    callable::BodyCallableCandidate,
    field::BodyFieldQuery,
    function::BodyFunctionQuery,
    generics::BodyGenericsQuery,
    impls::{BodyImplQuery, BodyReceiverImplMatches},
    traits::BodyTraitQuery,
    type_alias::BodyTypeAliasQuery,
    type_context::BodyTypeContextQuery,
    type_ref::TypeRefResolutionQuery,
};

pub(crate) use self::call::{BodyCallQuery, CallProjection, ResolvedCallTarget};
