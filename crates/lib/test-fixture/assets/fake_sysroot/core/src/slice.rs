pub struct Iter<'a, T>(&'a T);

impl<'a, T> crate::iter::Iterator for Iter<'a, T> {
    type Item = &'a T;
}

impl<'a, T> crate::iter::IntoIterator for &'a [T] {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
}
