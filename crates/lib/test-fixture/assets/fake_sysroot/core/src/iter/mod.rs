pub mod adapters;

pub use self::adapters::{Enumerate, Filter, FilterMap, Map, Skip};

pub trait FromIterator<A> {}

pub trait IntoIterator {
    type Item;
    type IntoIter;

    #[lang = "into_iter"]
    fn into_iter(self) -> Self::IntoIter;
}

#[lang = "iterator"]
pub trait Iterator {
    type Item;

    fn map<B, F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: crate::FnMut(Self::Item) -> B;

    fn filter<P>(self, predicate: P) -> Filter<Self, P>
    where
        Self: Sized,
        P: crate::FnMut(&Self::Item) -> bool;

    fn find<P>(&mut self, predicate: P) -> crate::option::Option<Self::Item>
    where
        Self: Sized,
        P: crate::FnMut(&Self::Item) -> bool;

    fn filter_map<B, F>(self, f: F) -> FilterMap<Self, F>
    where
        Self: Sized,
        F: crate::FnMut(Self::Item) -> crate::option::Option<B>;

    fn enumerate(self) -> Enumerate<Self>
    where
        Self: Sized;

    fn skip(self, n: usize) -> Skip<Self>
    where
        Self: Sized;

    fn collect<B: FromIterator<Self::Item>>(self) -> B
    where
        Self: Sized;
}

impl<I: Iterator> IntoIterator for I {
    type Item = I::Item;
    type IntoIter = I;

    fn into_iter(self) -> Self::IntoIter {}
}
