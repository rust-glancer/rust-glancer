pub struct Global;

pub trait Allocator {}

impl Allocator for Global {}

pub struct Vec<T, A = Global> {
    value: T,
    allocator: A,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}

    pub fn push(&mut self, _value: T) {}
}

impl<T, A: Allocator> core::ops::Deref for Vec<T, A> {
    type Target = [T];
}

impl<T> core::iter::FromIterator<T> for Vec<T> {}
