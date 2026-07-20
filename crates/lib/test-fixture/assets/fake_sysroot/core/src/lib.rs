extern crate self as core;

pub mod array;
pub mod fmt;
pub mod iter;
mod macros;
pub mod ops;
pub mod option;
pub mod prelude;
pub mod result;
pub mod slice;

pub use option::Option;
pub use result::Result;

impl<T> [T] {
    pub fn iter(&self) -> slice::Iter<'_, T> {}
}

impl str {
    pub fn starts_with(&self, _prefix: &str) -> bool {}
}

#[lang = "fn_once"]
pub trait FnOnce<Args> {
    #[lang = "fn_once_output"]
    type Output;
}

#[lang = "fn_mut"]
pub trait FnMut<Args>: FnOnce<Args> {}

#[lang = "fn"]
pub trait Fn<Args>: FnMut<Args> {}
