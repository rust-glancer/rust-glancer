pub struct IntoIter<T, const N: usize>([T; N]);

impl<T, const N: usize> crate::iter::Iterator for IntoIter<T, N> {
    type Item = T;
}

impl<T, const N: usize> crate::iter::IntoIterator for [T; N] {
    type Item = T;
    type IntoIter = IntoIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {}
}
