pub struct Arc<T> {
    value: T,
}

impl<T> Arc<T> {
    pub fn new(value: T) -> Self {}
}

impl<T> core::ops::Deref for Arc<T> {
    type Target = T;
}
