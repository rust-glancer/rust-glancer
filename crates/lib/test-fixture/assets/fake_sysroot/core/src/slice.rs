pub struct Iter<'a, T>(&'a T);

impl<'a, T> crate::iter::Iterator for Iter<'a, T> {
    type Item = &'a T;
}

impl<'a, T> crate::iter::IntoIterator for &'a [T] {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {}
}

// Keep the intermediate associated projection: slices and strings delegate their output
// type to the index. Bounds checks and access methods do not affect these relationships.
pub trait SliceIndex<T: ?crate::marker::Sized> {
    type Output: ?crate::marker::Sized;
}

impl<T, I: SliceIndex<[T]>> crate::ops::Index<I> for [T] {
    type Output = I::Output;
    fn index(&self, index: I) -> &Self::Output { loop {} }
}

impl<I: SliceIndex<str>> crate::ops::Index<I> for str {
    type Output = I::Output;
    fn index(&self, index: I) -> &Self::Output { loop {} }
}

impl<T> SliceIndex<[T]> for usize { type Output = T; }
impl<T> SliceIndex<[T]> for crate::ops::Range<usize> { type Output = [T]; }
impl<T> SliceIndex<[T]> for crate::ops::RangeFrom<usize> { type Output = [T]; }
impl<T> SliceIndex<[T]> for crate::ops::RangeTo<usize> { type Output = [T]; }
impl<T> SliceIndex<[T]> for crate::ops::RangeInclusive<usize> { type Output = [T]; }
impl<T> SliceIndex<[T]> for crate::ops::RangeToInclusive<usize> { type Output = [T]; }
impl<T> SliceIndex<[T]> for crate::ops::RangeFull { type Output = [T]; }

impl SliceIndex<str> for crate::ops::Range<usize> { type Output = str; }
impl SliceIndex<str> for crate::ops::RangeFrom<usize> { type Output = str; }
impl SliceIndex<str> for crate::ops::RangeTo<usize> { type Output = str; }
impl SliceIndex<str> for crate::ops::RangeInclusive<usize> { type Output = str; }
impl SliceIndex<str> for crate::ops::RangeToInclusive<usize> { type Output = str; }
impl SliceIndex<str> for crate::ops::RangeFull { type Output = str; }
