/// Marker used by relaxed `?Sized` bounds in the fake sysroot.
#[lang = "sized"]
pub trait Sized {}

#[lang = "tuple_trait"]
pub trait Tuple {}

#[lang = "destruct"]
pub trait Destruct {}

#[lang = "send"]
pub unsafe auto trait Send {}

#[lang = "sync"]
pub unsafe auto trait Sync {}

// These impls exercise the exceptions to structural inference used by real core.
impl<T: ?Sized> !Send for *const T {}
impl<T: ?Sized> !Send for *mut T {}
impl<T: ?Sized> !Sync for *const T {}
impl<T: ?Sized> !Sync for *mut T {}
unsafe impl<T: Sync + ?Sized> Send for &T {}

pub trait Copy: crate::clone::Clone {}
#[rustc_builtin_macro]
pub macro Copy($item:item) {}
