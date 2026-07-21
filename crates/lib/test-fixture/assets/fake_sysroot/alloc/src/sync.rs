pub struct Arc<T: ?core::marker::Sized>;

impl<T> Arc<T> {
    pub fn new(value: T) -> Self {}
}

impl<T: ?core::marker::Sized> core::ops::Deref for Arc<T> {
    type Target = T;
}
