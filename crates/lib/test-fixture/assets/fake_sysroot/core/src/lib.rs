extern crate self as core;

pub mod array;
pub mod cell;
pub mod fmt;
pub mod iter;
pub mod marker;
mod macros;
pub mod ops;
pub mod option;
pub mod prelude;
pub mod result;
pub mod slice;

pub use option::Option;
pub use result::Result;

// Trait signatures and compiler hooks used by builtin derive fixtures. Keeping the hooks in
// their normal modules exercises the same imports as real core, including macro aliases.
pub mod clone {
    pub trait Clone: crate::marker::Sized {
        fn clone(&self) -> Self;
        fn clone_from(&mut self, source: &Self) {}
    }
    #[rustc_builtin_macro]
    pub macro Clone($item:item) {}
}

pub mod default {
    pub trait Default: crate::marker::Sized {
        fn default() -> Self;
    }
    #[rustc_builtin_macro]
    pub macro Default($item:item) {}
}

pub mod cmp {
    pub enum Ordering { Less, Equal, Greater }
    pub trait PartialEq<Rhs = Self> {
        fn eq(&self, other: &Rhs) -> bool;
    }
    pub trait Eq: PartialEq<Self> {}
    pub trait PartialOrd<Rhs = Self>: PartialEq<Rhs> {
        fn partial_cmp(&self, other: &Rhs) -> crate::option::Option<Ordering>;
    }
    pub trait Ord: Eq + PartialOrd<Self> {
        fn cmp(&self, other: &Self) -> Ordering;
    }
    #[rustc_builtin_macro]
    pub macro PartialEq($item:item) {}
    #[rustc_builtin_macro]
    pub macro Eq($item:item) {}
    #[rustc_builtin_macro]
    pub macro PartialOrd($item:item) {}
    #[rustc_builtin_macro]
    pub macro Ord($item:item) {}
}

pub mod hash {
    pub trait Hasher {}
    pub trait Hash {
        fn hash<H: Hasher>(&self, state: &mut H);
    }
    pub mod macros {
        #[rustc_builtin_macro]
        pub macro Hash($item:item) {}
    }
    pub use macros::Hash;
}

impl<T> [T] {
    pub fn iter(&self) -> slice::Iter<'_, T> {}
}

impl str {
    pub fn starts_with(&self, _prefix: &str) -> bool {}
}

#[lang = "fn_once"]
pub trait FnOnce<Args: marker::Tuple> {
    #[lang = "fn_once_output"]
    type Output;
}

#[lang = "fn_mut"]
pub trait FnMut<Args: marker::Tuple>: FnOnce<Args> {}

#[lang = "fn"]
pub trait Fn<Args: marker::Tuple>: FnMut<Args> {}
