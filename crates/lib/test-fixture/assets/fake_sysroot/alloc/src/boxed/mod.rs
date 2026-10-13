#[lang = "owned_box"]
pub struct Box<T: ?core::marker::Sized>;

impl<T> Box<T> {
    pub fn new(value: T) -> Self {}
}

impl<T: ?core::marker::Sized> core::ops::Deref for Box<T> {
    type Target = T;
}
