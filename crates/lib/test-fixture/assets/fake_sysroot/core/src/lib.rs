extern crate self as core;

pub mod iter;
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

pub trait FnOnce<Args> {
    type Output;
}

pub trait FnMut<Args>: FnOnce<Args> {}

pub trait Fn<Args>: FnMut<Args> {}
