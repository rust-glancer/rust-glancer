pub struct Map<I, F> {
    iter: I,
    f: F,
}

pub struct Filter<I, P> {
    iter: I,
    predicate: P,
}

pub struct FilterMap<I, F> {
    iter: I,
    f: F,
}

pub struct Enumerate<I> {
    iter: I,
}

impl<B, I: crate::iter::Iterator, F> crate::iter::Iterator for Map<I, F>
where
    F: crate::FnMut(I::Item) -> B,
{
    type Item = B;
}

impl<I: crate::iter::Iterator, P> crate::iter::Iterator for Filter<I, P>
where
    P: crate::FnMut(&I::Item) -> bool,
{
    type Item = I::Item;
}

impl<B, I: crate::iter::Iterator, F> crate::iter::Iterator for FilterMap<I, F>
where
    F: crate::FnMut(I::Item) -> crate::option::Option<B>,
{
    type Item = B;
}

impl<I: crate::iter::Iterator> crate::iter::Iterator for Enumerate<I> {
    type Item = (usize, I::Item);
}
