pub struct RefCell<T> {
    value: T,
}

impl<T> RefCell<T> {
    pub fn borrow(&self) -> Ref<'_, T> {}
}

pub struct Ref<'a, T: ?crate::marker::Sized> {
    value: &'a T,
}

impl<'a, T: ?crate::marker::Sized> crate::ops::Deref for Ref<'a, T> {
    type Target = T;
}
