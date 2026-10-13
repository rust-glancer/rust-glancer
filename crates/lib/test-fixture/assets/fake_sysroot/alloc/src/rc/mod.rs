#[rustc_diagnostic_item = "Rc"]
pub struct Rc<T: ?core::marker::Sized>;

impl<T> Rc<T> {
    pub fn new(value: T) -> Self {}
}

impl<T: ?core::marker::Sized> core::ops::Deref for Rc<T> {
    type Target = T;
}
