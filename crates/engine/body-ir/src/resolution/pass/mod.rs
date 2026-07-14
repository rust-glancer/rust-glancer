mod body;
mod builtin_macro;
mod env;
mod expr;
mod inference;
mod pattern_binding;
mod pattern_type;
mod ty_normalize;

pub(crate) use self::body::BodyResolutionPass;
