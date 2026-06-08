use std::slice;

use crate::{MemoryRecorder, MemorySize, Shrink};

/// Vec-backed ordered set for small candidate lists.
///
/// This preserves insertion order and uses linear equality checks. It is intended for small
/// compiler-style candidate lists where discovery order matters more than hash-table throughput.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueVec<T> {
    items: Vec<T>,
}

impl<T> Default for UniqueVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> UniqueVec<T> {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.items
    }
}

impl<T> UniqueVec<T>
where
    T: PartialEq,
{
    /// Adds `item` when no equal item has been seen yet.
    ///
    /// Returns whether the item was inserted.
    pub fn push(&mut self, item: T) -> bool {
        if self.items.contains(&item) {
            return false;
        }

        self.items.push(item);
        true
    }
}

impl<T> Extend<T> for UniqueVec<T>
where
    T: PartialEq,
{
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        for item in iter {
            self.push(item);
        }
    }
}

impl<T> FromIterator<T> for UniqueVec<T>
where
    T: PartialEq,
{
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let mut items = Self::new();
        items.extend(iter);
        items
    }
}

impl<T> IntoIterator for UniqueVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a UniqueVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<T> From<UniqueVec<T>> for Vec<T> {
    fn from(value: UniqueVec<T>) -> Self {
        value.into_vec()
    }
}

impl<T> MemorySize for UniqueVec<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.items.record_memory_children(recorder);
    }
}

impl<T> Shrink for UniqueVec<T>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(&mut self.items);
    }
}

#[cfg(test)]
mod tests {
    use crate::{MemoryRecordKind, MemoryRecorder, MemorySize, Shrink, UniqueVec};

    #[test]
    fn push_preserves_first_seen_order_and_reports_insertions() {
        let mut values = UniqueVec::new();

        assert!(values.push("core"));
        assert!(values.push("alloc"));
        assert!(!values.push("core"));
        assert!(values.push("std"));

        assert_eq!(values.as_slice(), &["core", "alloc", "std"]);
    }

    #[test]
    fn extend_and_collect_keep_only_first_occurrences() {
        let mut values = UniqueVec::with_capacity(4);
        values.extend(["core", "alloc", "core"]);

        let collected = ["std", "core", "std", "alloc"]
            .into_iter()
            .collect::<UniqueVec<_>>();

        assert_eq!(values.into_vec(), vec!["core", "alloc"]);
        assert_eq!(collected.into_vec(), vec!["std", "core", "alloc"]);
    }

    #[test]
    fn memory_size_delegates_to_inner_vec() {
        let values = ["core".to_owned(), "core".to_owned()]
            .into_iter()
            .collect::<UniqueVec<_>>();

        let mut recorder = MemoryRecorder::new("unique");
        values.record_memory_size(&mut recorder);

        let totals = recorder.totals_by_kind();
        assert!(
            totals
                .get(&MemoryRecordKind::Heap)
                .is_some_and(|bytes| *bytes >= "core".len())
        );
    }

    #[test]
    fn shrink_delegates_to_inner_items() {
        let mut values = UniqueVec::new();
        values.push("core".to_owned());

        Shrink::shrink_to_fit(&mut values);

        assert_eq!(values.as_slice(), &["core"]);
    }
}
