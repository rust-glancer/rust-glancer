mod body_items;
mod callable;
mod receiver_items;
mod scope;
mod type_path;
mod type_ref;
mod value_path;

pub(crate) use self::{
    body_items::BodyLocalItemQuery, receiver_items::BodyReceiverFunctionQuery,
    type_path::BodyTypePathQuery, type_ref::TypeRefUseSite, value_path::BodyValuePathQuery,
};

pub use self::scope::BodyScopeQuery;

pub(crate) use self::callable::CallableReturnQuery;
