pub struct IntoIter<T, const N: usize>([T; N]);

impl<T, I, const N: usize> crate::ops::Index<I> for [T; N]
where
    [T]: crate::ops::Index<I>,
{
    type Output = <[T] as crate::ops::Index<I>>::Output;
    fn index(&self, index: I) -> &Self::Output { loop {} }
}

impl<T, const N: usize> crate::iter::Iterator for IntoIter<T, N> {
    type Item = T;
}

impl<T, const N: usize> crate::iter::IntoIterator for [T; N] {
    type Item = T;
    type IntoIter = IntoIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {}
}
