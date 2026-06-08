use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::OsString,
    hash::{BuildHasher, Hash},
    mem,
    path::PathBuf,
    sync::Arc,
};

use crate::Shrink;

use super::{MemoryRecorder, MemorySize, approximate_allocation_overhead};

super::impl_memory_size_leaf!(
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

impl<T> MemorySize for &T {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl<T> MemorySize for Option<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        if let Some(value) = self {
            recorder.scope("some", |recorder| value.record_memory_children(recorder));
        }
    }
}

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

impl<T, E> MemorySize for Result<T, E>
where
    T: MemorySize,
    E: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Ok(value) => recorder.scope("ok", |recorder| value.record_memory_children(recorder)),
            Err(error) => recorder.scope("err", |recorder| error.record_memory_children(recorder)),
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

impl<T> MemorySize for Box<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("box", |recorder| {
            let payload = mem::size_of::<T>();
            recorder.record_heap::<T>(payload);
            recorder.record_approximate::<Box<T>>(approximate_allocation_overhead(payload));
            (**self).record_memory_children(recorder);
        });
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

impl<T> MemorySize for Arc<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("arc", |recorder| {
            let payload = mem::size_of::<T>();
            recorder.record_heap::<T>(payload);
            recorder.record_approximate::<Arc<T>>(approximate_allocation_overhead(payload));
            (**self).record_memory_children(recorder);
        });
    }
}

impl<T> MemorySize for Box<[T]>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.len().saturating_mul(mem::size_of::<T>());
        recorder.scope("items", |recorder| {
            recorder.record_heap::<T>(payload);

            for item in self.iter() {
                item.record_memory_children(recorder);
            }
        });
        recorder.scope("allocation_overhead", |recorder| {
            recorder.record_approximate::<Box<[T]>>(approximate_allocation_overhead(payload));
        });
    }
}

impl MemorySize for Box<str> {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.len();
        recorder.record_heap::<str>(payload);
        recorder.record_approximate::<Box<str>>(approximate_allocation_overhead(payload));
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

impl<T> MemorySize for Vec<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.capacity().saturating_mul(mem::size_of::<T>());
        recorder.scope("items", |recorder| {
            recorder.record_heap::<T>(self.len().saturating_mul(mem::size_of::<T>()));

            for item in self {
                item.record_memory_children(recorder);
            }
        });

        let spare = self.capacity().saturating_sub(self.len());
        recorder.scope("spare_capacity", |recorder| {
            recorder.record_spare_capacity::<T>(spare.saturating_mul(mem::size_of::<T>()));
        });
        recorder.scope("allocation_overhead", |recorder| {
            recorder.record_approximate::<Vec<T>>(approximate_allocation_overhead(payload));
        });
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

impl<T, const N: usize> MemorySize for [T; N]
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("items", |recorder| {
            for item in self {
                item.record_memory_children(recorder);
            }
        });
    }
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

impl<A, B> MemorySize for (A, B)
where
    A: MemorySize,
    B: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("0", |recorder| self.0.record_memory_children(recorder));
        recorder.scope("1", |recorder| self.1.record_memory_children(recorder));
    }
}

impl<A, B, C> MemorySize for (A, B, C)
where
    A: MemorySize,
    B: MemorySize,
    C: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("0", |recorder| self.0.record_memory_children(recorder));
        recorder.scope("1", |recorder| self.1.record_memory_children(recorder));
        recorder.scope("2", |recorder| self.2.record_memory_children(recorder));
    }
}

impl<A, B, C, D> MemorySize for (A, B, C, D)
where
    A: MemorySize,
    B: MemorySize,
    C: MemorySize,
    D: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("0", |recorder| self.0.record_memory_children(recorder));
        recorder.scope("1", |recorder| self.1.record_memory_children(recorder));
        recorder.scope("2", |recorder| self.2.record_memory_children(recorder));
        recorder.scope("3", |recorder| self.3.record_memory_children(recorder));
    }
}

impl MemorySize for String {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let initialized = self.len();
        let spare = self.capacity().saturating_sub(self.len());

        recorder.record_heap::<str>(initialized);
        recorder.record_spare_capacity::<String>(spare);
        let payload = self.capacity();
        recorder.record_approximate::<String>(approximate_allocation_overhead(payload));
    }
}

impl Shrink for String {
    fn shrink_to_fit(&mut self) {
        String::shrink_to_fit(self);
    }
}

impl MemorySize for OsString {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.as_encoded_bytes().len();
        recorder.record_approximate::<OsString>(
            payload.saturating_add(approximate_allocation_overhead(payload)),
        );
    }
}

impl MemorySize for PathBuf {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.as_os_str().as_encoded_bytes().len();
        recorder.record_approximate::<PathBuf>(
            payload.saturating_add(approximate_allocation_overhead(payload)),
        );
    }
}

impl<'a, B> MemorySize for Cow<'a, B>
where
    B: ToOwned + ?Sized,
    B::Owned: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Cow::Borrowed(_) => {}
            Cow::Owned(value) => value.record_memory_children(recorder),
        }
    }
}

impl<K, V, S> MemorySize for HashMap<K, V, S>
where
    K: MemorySize + Eq + Hash,
    V: MemorySize,
    S: BuildHasher,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self
            .capacity()
            .saturating_mul(mem::size_of::<K>().saturating_add(mem::size_of::<V>()));
        let control_bytes = hash_table_control_bytes(self.capacity());

        recorder.scope("entries", |recorder| {
            // HashMap hides bucket/control-byte layout. Key/value payload bytes are useful as
            // heap attribution; spare slot storage remains approximate.
            recorder.record_heap::<K>(self.len().saturating_mul(mem::size_of::<K>()));
            recorder.record_heap::<V>(self.len().saturating_mul(mem::size_of::<V>()));

            for (key, value) in self {
                recorder.scope("key", |recorder| key.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
        });

        let spare = self.capacity().saturating_sub(self.len());
        recorder.scope("spare_capacity", |recorder| {
            recorder.record_approximate::<HashMap<K, V, S>>(
                spare.saturating_mul(mem::size_of::<K>().saturating_add(mem::size_of::<V>())),
            );
        });
        recorder.scope("table_overhead", |recorder| {
            recorder.record_approximate::<HashMap<K, V, S>>(
                control_bytes
                    .saturating_add(approximate_allocation_overhead(payload + control_bytes)),
            );
        });
    }
}

impl<T, S> MemorySize for HashSet<T, S>
where
    T: MemorySize + Eq + Hash,
    S: BuildHasher,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let payload = self.capacity().saturating_mul(mem::size_of::<T>());
        let control_bytes = hash_table_control_bytes(self.capacity());

        recorder.scope("items", |recorder| {
            recorder.record_heap::<T>(self.len().saturating_mul(mem::size_of::<T>()));

            for item in self {
                item.record_memory_children(recorder);
            }
        });

        let spare = self.capacity().saturating_sub(self.len());
        recorder.scope("spare_capacity", |recorder| {
            recorder.record_approximate::<HashSet<T, S>>(spare.saturating_mul(mem::size_of::<T>()));
        });
        recorder.scope("table_overhead", |recorder| {
            recorder.record_approximate::<HashSet<T, S>>(
                control_bytes
                    .saturating_add(approximate_allocation_overhead(payload + control_bytes)),
            );
        });
    }
}

fn hash_table_control_bytes(capacity: usize) -> usize {
    if capacity == 0 {
        0
    } else {
        // std::HashMap is backed by hashbrown's SwissTable. The exact bucket count is hidden, but
        // one control byte per public capacity slot plus one SIMD group is a useful lower-bound.
        capacity.saturating_add(16)
    }
}

impl<K, V> MemorySize for BTreeMap<K, V>
where
    K: MemorySize,
    V: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("entries", |recorder| {
            // BTree node layout is private, so entry payload storage is intentionally approximate.
            recorder.record_approximate::<BTreeMap<K, V>>(
                self.len()
                    .saturating_mul(mem::size_of::<K>().saturating_add(mem::size_of::<V>())),
            );

            for (key, value) in self {
                recorder.scope("key", |recorder| key.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
        });
    }
}

impl<T> MemorySize for BTreeSet<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("items", |recorder| {
            recorder
                .record_approximate::<BTreeSet<T>>(self.len().saturating_mul(mem::size_of::<T>()));

            for item in self {
                item.record_memory_children(recorder);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{any, collections::BTreeMap, mem, sync::Arc};

    use crate::memsize::{
        MemoryRecordKind, MemoryRecorder, MemorySize, approximate_allocation_overhead,
    };

    #[test]
    fn records_string_shallow_and_heap_capacity() {
        let mut value = String::with_capacity(16);
        value.push_str("api");

        assert_eq!(
            value.memory_size(),
            mem::size_of::<String>() + 16 + approximate_allocation_overhead(16)
        );

        let mut recorder = MemoryRecorder::new("string");
        value.record_memory_size(&mut recorder);

        let totals = recorder.totals_by_kind();
        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<String>())
        );
        assert_eq!(totals.get(&MemoryRecordKind::Heap), Some(&3));
        assert_eq!(totals.get(&MemoryRecordKind::SpareCapacity), Some(&13));
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(&approximate_allocation_overhead(16))
        );
    }

    #[test]
    fn option_records_inline_value_once_but_keeps_owned_children() {
        let mut value = String::with_capacity(11);
        value.push_str("user");
        let value = Some(value);

        assert_eq!(
            value.memory_size(),
            mem::size_of::<Option<String>>() + 11 + approximate_allocation_overhead(11)
        );

        let mut recorder = MemoryRecorder::new("option");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Option<String>>())
        );
        assert_eq!(totals.get(&MemoryRecordKind::Heap), Some(&4));
        assert_eq!(totals.get(&MemoryRecordKind::SpareCapacity), Some(&7));
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(&approximate_allocation_overhead(11))
        );
    }

    #[test]
    fn box_records_pointee_storage_as_heap() {
        let mut value = String::with_capacity(5);
        value.push_str("tool");
        let value = Box::new(value);

        assert_eq!(
            value.memory_size(),
            mem::size_of::<Box<String>>()
                + mem::size_of::<String>()
                + 5
                + approximate_allocation_overhead(mem::size_of::<String>())
                + approximate_allocation_overhead(5)
        );

        let mut recorder = MemoryRecorder::new("box");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Box<String>>())
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::Heap),
            Some(&(mem::size_of::<String>() + 4))
        );
        assert_eq!(totals.get(&MemoryRecordKind::SpareCapacity), Some(&1));
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(
                &(approximate_allocation_overhead(mem::size_of::<String>())
                    + approximate_allocation_overhead(5))
            )
        );
    }

    #[test]
    fn arc_records_pointee_storage_as_heap() {
        let mut value = String::with_capacity(5);
        value.push_str("tool");
        let value = Arc::new(value);

        assert_eq!(
            value.memory_size(),
            mem::size_of::<Arc<String>>()
                + mem::size_of::<String>()
                + 5
                + approximate_allocation_overhead(mem::size_of::<String>())
                + approximate_allocation_overhead(5)
        );

        let mut recorder = MemoryRecorder::new("arc");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Arc<String>>())
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::Heap),
            Some(&(mem::size_of::<String>() + 4))
        );
        assert_eq!(totals.get(&MemoryRecordKind::SpareCapacity), Some(&1));
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(
                &(approximate_allocation_overhead(mem::size_of::<String>())
                    + approximate_allocation_overhead(5))
            )
        );
    }

    #[test]
    fn vec_records_initialized_items_as_heap_and_spare_capacity_separately() {
        let mut item = String::with_capacity(5);
        item.push_str("tool");
        let mut value = Vec::with_capacity(2);
        value.push(item);

        assert_eq!(
            value.memory_size(),
            mem::size_of::<Vec<String>>()
                + 2 * mem::size_of::<String>()
                + 5
                + approximate_allocation_overhead(2 * mem::size_of::<String>())
                + approximate_allocation_overhead(5)
        );

        let mut recorder = MemoryRecorder::new("vec");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Vec<String>>())
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::Heap),
            Some(&(mem::size_of::<String>() + 4))
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::SpareCapacity),
            Some(&(mem::size_of::<String>() + 1))
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(
                &(approximate_allocation_overhead(2 * mem::size_of::<String>())
                    + approximate_allocation_overhead(5))
            )
        );
    }

    #[test]
    fn boxed_slice_records_exact_payload_without_spare_capacity() {
        let value = vec![1_u32, 2, 3].into_boxed_slice();

        let mut recorder = MemoryRecorder::new("slice");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Box<[u32]>>())
        );
        assert_eq!(
            totals.get(&MemoryRecordKind::Heap),
            Some(&(3 * mem::size_of::<u32>()))
        );
        assert!(!totals.contains_key(&MemoryRecordKind::SpareCapacity));
    }

    #[test]
    fn boxed_str_records_exact_payload_without_spare_capacity() {
        let value: Box<str> = "crate".to_string().into_boxed_str();

        let mut recorder = MemoryRecorder::new("str");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Box<str>>())
        );
        assert_eq!(totals.get(&MemoryRecordKind::Heap), Some(&5));
        assert!(!totals.contains_key(&MemoryRecordKind::SpareCapacity));
    }

    #[test]
    fn vec_records_element_type_names() {
        let mut value = Vec::with_capacity(3);
        value.push(10_u32);
        value.push(20_u32);

        let mut recorder = MemoryRecorder::new("vec");
        value.record_memory_size(&mut recorder);

        let totals = recorder.totals_by_type();
        assert_eq!(
            totals.get(any::type_name::<Vec<u32>>()),
            Some(
                &(mem::size_of::<Vec<u32>>()
                    + approximate_allocation_overhead(3 * mem::size_of::<u32>()))
            )
        );
        assert_eq!(
            totals.get(any::type_name::<u32>()),
            Some(&(3 * mem::size_of::<u32>()))
        );
    }

    #[test]
    fn tuple_records_owned_children() {
        let mut text = String::with_capacity(9);
        text.push_str("module");
        let value = (7_u32, text);

        let mut recorder = MemoryRecorder::new("tuple");
        value.record_memory_size(&mut recorder);

        let totals = recorder.totals_by_kind();
        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<(u32, String)>())
        );
        assert_eq!(totals.get(&MemoryRecordKind::Heap), Some(&6));
        assert_eq!(totals.get(&MemoryRecordKind::SpareCapacity), Some(&3));
        assert_eq!(
            totals.get(&MemoryRecordKind::Approximate),
            Some(&approximate_allocation_overhead(9))
        );
    }

    #[test]
    fn map_records_hidden_capacity_as_approximate() {
        let mut value = BTreeMap::new();
        value.insert("one".to_owned(), "two".to_owned());

        let mut recorder = MemoryRecorder::new("map");
        value.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert!(totals.contains_key(&MemoryRecordKind::Approximate));
        assert!(totals.contains_key(&MemoryRecordKind::Heap));
    }
}
