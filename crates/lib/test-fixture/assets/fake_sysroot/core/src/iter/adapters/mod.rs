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

pub struct Skip<I> {
    iter: I,
}

impl<B, I: crate::iter::Iterator, F> crate::iter::Iterator for Map<I, F>
where
    F: crate::FnMut(<I as crate::iter::Iterator>::Item) -> B,
{
    type Item = B;
}

impl<I: crate::iter::Iterator, P> crate::iter::Iterator for Filter<I, P>
where
    P: crate::FnMut(&<I as crate::iter::Iterator>::Item) -> bool,
{
    type Item = <I as crate::iter::Iterator>::Item;
}

impl<B, I: crate::iter::Iterator, F> crate::iter::Iterator for FilterMap<I, F>
where
    F: crate::FnMut(<I as crate::iter::Iterator>::Item) -> crate::option::Option<B>,
{
    type Item = B;
}

impl<I: crate::iter::Iterator> crate::iter::Iterator for Enumerate<I> {
    type Item = (usize, <I as crate::iter::Iterator>::Item);
}

impl<I> crate::iter::Iterator for Skip<I>
where
    I: crate::iter::Iterator,
{
    type Item = <I as crate::iter::Iterator>::Item;
}
