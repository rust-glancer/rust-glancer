pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}

    pub fn push(&mut self, _value: T) {}
}

impl<T> core::iter::FromIterator<T> for Vec<T> {}

