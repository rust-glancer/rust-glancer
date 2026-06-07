mod body_items;
mod callable;
mod receiver_items;
mod scope;
mod type_path;
mod type_ref;
mod value_path;

pub(crate) use self::{
    body_items::BodyLocalItemQuery, receiver_items::BodyReceiverFunctionQuery,
    type_path::BodyTypePathResolver, type_ref::TypeRefUseSite, value_path::BodyValuePathResolver,
};

pub use self::scope::BodyScopeQuery;

pub(crate) use self::callable::CallableReturnResolver;
