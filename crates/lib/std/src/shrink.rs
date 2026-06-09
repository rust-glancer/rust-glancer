pub use rg_std_derive::Shrink;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    hash::{BuildHasher, Hash},
};

/// Releases spare heap capacity retained inside a value.
///
/// This is intentionally separate from `MemorySize`: some generic data models need to ask their
/// embedded storage-specific values to compact themselves without also knowing how those values
/// report memory usage.
pub trait Shrink {
    fn shrink_to_fit(&mut self);
}

macro_rules! impl_shrink_leaf {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Shrink for $ty {
                fn shrink_to_fit(&mut self) {}
            }
        )+
    };
}

impl_shrink_leaf!(
    (),
    bool,
    char,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
);

impl<T> Shrink for Option<T>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        if let Some(value) = self {
            value.shrink_to_fit();
        }
    }
}

impl<T, E> Shrink for Result<T, E>
where
    T: Shrink,
    E: Shrink,
{
    fn shrink_to_fit(&mut self) {
        match self {
            Ok(value) => value.shrink_to_fit(),
            Err(error) => error.shrink_to_fit(),
        }
    }
}

impl<T> Shrink for Box<T>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        (**self).shrink_to_fit();
    }
}

impl<T> Shrink for Box<[T]>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        for item in self.iter_mut() {
            item.shrink_to_fit();
        }
    }
}

impl<T> Shrink for Vec<T>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        Vec::shrink_to_fit(self);
        for item in self {
            item.shrink_to_fit();
        }
    }
}

// Hash keys and set elements are lookup identities, so the default impl only compacts the table
// storage. Values are safe to shrink because mutating them cannot invalidate the bucket layout.
impl<K, V, S> Shrink for HashMap<K, V, S>
where
    K: Eq + Hash,
    V: Shrink,
    S: BuildHasher,
{
    fn shrink_to_fit(&mut self) {
        HashMap::shrink_to_fit(self);
        for value in self.values_mut() {
            value.shrink_to_fit();
        }
    }
}

impl<T, S> Shrink for HashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    fn shrink_to_fit(&mut self) {
        HashSet::shrink_to_fit(self);
    }
}

impl<K, V> Shrink for BTreeMap<K, V>
where
    V: Shrink,
{
    fn shrink_to_fit(&mut self) {
        for value in self.values_mut() {
            value.shrink_to_fit();
        }
    }
}

impl<T> Shrink for BTreeSet<T> {
    fn shrink_to_fit(&mut self) {}
}

impl<T, const N: usize> Shrink for [T; N]
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        for item in self {
            item.shrink_to_fit();
        }
    }
}

impl<A, B> Shrink for (A, B)
where
    A: Shrink,
    B: Shrink,
{
    fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit();
        self.1.shrink_to_fit();
    }
}

impl Shrink for String {
    fn shrink_to_fit(&mut self) {
        String::shrink_to_fit(self);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

    use crate::Shrink;

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Probe {
        calls: usize,
    }

    impl Shrink for Probe {
        fn shrink_to_fit(&mut self) {
            self.calls += 1;
        }
    }

    #[test]
    fn container_impls_shrink_owned_children() {
        let mut option = Some(Probe::default());
        option.shrink_to_fit();
        assert_eq!(option.expect("option should stay populated").calls, 1);

        let mut ok: Result<Probe, Probe> = Ok(Probe::default());
        ok.shrink_to_fit();
        assert_eq!(ok.expect("ok result should stay ok").calls, 1);

        let mut err: Result<Probe, Probe> = Err(Probe::default());
        err.shrink_to_fit();
        assert_eq!(err.expect_err("err result should stay err").calls, 1);

        let mut boxed = Box::new(Probe::default());
        boxed.shrink_to_fit();
        assert_eq!(boxed.calls, 1);

        let mut boxed_slice = vec![Probe::default(), Probe::default()].into_boxed_slice();
        boxed_slice.shrink_to_fit();
        assert_eq!(
            boxed_slice.iter().map(|probe| probe.calls).sum::<usize>(),
            2
        );

        let mut values = vec![Probe::default(), Probe::default(), Probe::default()];
        Shrink::shrink_to_fit(&mut values);
        assert_eq!(values.iter().map(|probe| probe.calls).sum::<usize>(), 3);

        let mut array = [Probe::default(), Probe::default()];
        array.shrink_to_fit();
        assert_eq!(array.iter().map(|probe| probe.calls).sum::<usize>(), 2);

        let mut pair = (Probe::default(), Probe::default());
        pair.shrink_to_fit();
        assert_eq!(pair.0.calls, 1);
        assert_eq!(pair.1.calls, 1);
    }

    #[test]
    fn map_impls_shrink_values_without_touching_keys() {
        let mut hash_map = HashMap::with_capacity(128);
        hash_map.insert("first", Probe::default());
        hash_map.insert("second", Probe::default());
        let hash_capacity = hash_map.capacity();

        Shrink::shrink_to_fit(&mut hash_map);
        assert!(hash_map.capacity() <= hash_capacity);
        assert_eq!(hash_map.values().map(|probe| probe.calls).sum::<usize>(), 2);

        let mut btree_map = BTreeMap::new();
        btree_map.insert("first", Probe::default());
        btree_map.insert("second", Probe::default());

        Shrink::shrink_to_fit(&mut btree_map);
        assert_eq!(
            btree_map.values().map(|probe| probe.calls).sum::<usize>(),
            2
        );
    }

    #[test]
    fn set_impls_shrink_collection_storage_only() {
        let mut hash_set = HashSet::with_capacity(128);
        hash_set.insert("first");
        hash_set.insert("second");
        let hash_capacity = hash_set.capacity();

        Shrink::shrink_to_fit(&mut hash_set);
        assert!(hash_set.capacity() <= hash_capacity);
        assert!(hash_set.contains("first"));
        assert!(hash_set.contains("second"));

        let mut btree_set = BTreeSet::new();
        btree_set.insert("first");
        btree_set.insert("second");

        Shrink::shrink_to_fit(&mut btree_set);
        assert!(btree_set.contains("first"));
        assert!(btree_set.contains("second"));
    }
}
