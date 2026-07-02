pub mod adapters;

pub use self::adapters::{Enumerate, Filter, FilterMap, Map};

pub trait FromIterator<A> {}

pub trait IntoIterator {
    type Item;
    type IntoIter;
}

pub trait Iterator {
    type Item;

    fn map<B, F>(self, f: F) -> Map<Self, F>
    where
        F: crate::FnMut(Self::Item) -> B;

    fn filter<P>(self, predicate: P) -> Filter<Self, P>
    where
        P: crate::FnMut(&Self::Item) -> bool;

    fn filter_map<B, F>(self, f: F) -> FilterMap<Self, F>
    where
        F: crate::FnMut(Self::Item) -> crate::option::Option<B>;

    fn enumerate(self) -> Enumerate<Self>;

    fn collect<B>(self) -> B
    where
        B: FromIterator<Self::Item>;
}

impl<I: Iterator> IntoIterator for I {
    type Item = I::Item;
    type IntoIter = I;
}
