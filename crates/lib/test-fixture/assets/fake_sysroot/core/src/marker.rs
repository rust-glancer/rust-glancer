/// Marker used by relaxed `?Sized` bounds in the fake sysroot.
#[lang = "sized"]
pub trait Sized {}

#[lang = "tuple_trait"]
pub trait Tuple {}

#[lang = "destruct"]
pub trait Destruct {}
