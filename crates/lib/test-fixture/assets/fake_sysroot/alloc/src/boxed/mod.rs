#[lang = "owned_box"]
pub struct Box<T: ?core::marker::Sized>;

unsafe impl<T: core::marker::Send + ?core::marker::Sized> core::marker::Send for Box<T> {}
unsafe impl<T: core::marker::Sync + ?core::marker::Sized> core::marker::Sync for Box<T> {}

impl<T> Box<T> {
    pub fn new(value: T) -> Self {}
}

impl<T: ?core::marker::Sized> core::ops::Deref for Box<T> {
    type Target = T;
}
