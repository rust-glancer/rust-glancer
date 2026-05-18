//! Lightweight, approximate memory attribution for rust-glancer data structures.
//!
//! The goal is not allocator-perfect accounting. We want stable, tagged measurements that explain
//! which retained phase data deserves optimization attention.

use std::{any, collections::BTreeMap, mem};

mod default_impls;
#[cfg(feature = "ls_types")]
mod ls_types_impls;
#[cfg(feature = "rg_syntax")]
mod rg_syntax_impls;

/// Records approximate retained memory for a value.
///
/// Implementations follow a two-part convention:
/// - `record_memory_size` records the value's inline/shallow size;
/// - `record_memory_children` records memory owned behind pointers or container buffers.
///
/// Manual struct impls can usually call the default `record_memory_size` and then call
/// `record_memory_children` on inline fields to avoid double-counting those fields' shallow bytes.
/// If field-level attribution matters, override `record_memory_size` and record inline fields
/// explicitly, with any padding/unknown remainder tagged back to the parent type.
pub trait MemorySize {
    fn record_memory_size(&self, recorder: &mut MemoryRecorder)
    where
        Self: Sized,
    {
        recorder.record_shallow::<Self>(mem::size_of::<Self>());
        self.record_memory_children(recorder);
    }

    fn record_memory_children(&self, recorder: &mut MemoryRecorder);

    fn memory_size(&self) -> usize
    where
        Self: Sized,
    {
        let mut recorder = MemoryRecorder::total_only("root");
        self.record_memory_size(&mut recorder);
        recorder.total_bytes()
    }
}

/// Records memory children for named fields under scopes matching their field names.
///
/// This is the small building block for manual `MemorySize` impls where most fields can be
/// recorded mechanically, but the type still has one or two special cases that should stay
/// handwritten. Field-level attributes, such as `#[allow(deprecated)]`, can be attached before a
/// field when upstream structs expose legacy fields that still need to be counted.
///
/// ```
/// # use rg_memsize::{MemoryRecorder, MemorySize, record_memory_fields};
/// # struct Package { files: Vec<String>, target_roots: Vec<String> }
/// # impl MemorySize for Package {
/// #     fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
/// record_memory_fields!(recorder, self, files, target_roots);
/// #     }
/// # }
/// ```
#[macro_export]
macro_rules! record_memory_fields {
    ($recorder:expr, $owner:expr, $($(#[$field_attr:meta])* $field:ident),+ $(,)?) => {
        $(
            $(#[$field_attr])*
            {
                $recorder.scope(stringify!($field), |recorder| {
                    $crate::MemorySize::record_memory_children(&$owner.$field, recorder);
                });
            }
        )+
    };
}

/// Implements `MemorySize` for plain structs by recording the listed fields as children.
///
/// Use this for data types whose memory accounting is exactly "walk these fields". Keep manual
/// impls for enums, transparent wrappers, lazy/offloaded state, and anything with approximate or
/// intentionally omitted accounting.
///
/// ```
/// # use rg_memsize::{MemorySize, impl_memory_size_children};
/// # struct Package { files: Vec<String>, target_roots: Vec<String> }
/// # struct FileTree { file: String, docs: Vec<String>, items: Vec<String> }
/// impl_memory_size_children! {
///     Package => files, target_roots;
///     FileTree => file, docs, items;
/// }
/// ```
#[macro_export]
macro_rules! impl_memory_size_children {
    ($($ty:ty => $($(#[$field_attr:meta])* $field:ident),+ $(,)?);+ $(;)?) => {
        $(
            impl $crate::MemorySize for $ty {
                fn record_memory_children(&self, recorder: &mut $crate::MemoryRecorder) {
                    $crate::record_memory_fields!(
                        recorder,
                        self,
                        $($(#[$field_attr])* $field),+
                    );
                }
            }
        )+
    };
}

/// Implements `MemorySize` for leaf values that own no child allocations.
///
/// This is intended for small ids, flags, and enum-like marker types where `size_of::<T>()` is the
/// whole accounting story and the default `record_memory_size` shallow record is enough.
#[macro_export]
macro_rules! impl_memory_size_leaf {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl $crate::MemorySize for $ty {
                fn record_memory_children(&self, _recorder: &mut $crate::MemoryRecorder) {}
            }
        )+
    };
}

const ALLOCATION_HEADER_BYTES: usize = mem::size_of::<usize>() * 2;
const ALLOCATION_GRANULARITY: usize = mem::size_of::<usize>() * 2;

/// Best-effort size-class approximation for one heap allocation.
///
/// The exact allocator layout is intentionally outside this crate's scope, but RSS-oriented
/// profiles are misleading if millions of tiny allocations count only their logical payload bytes.
pub(crate) fn approximate_allocation_size(payload_bytes: usize) -> usize {
    if payload_bytes == 0 {
        return 0;
    }

    round_up(
        payload_bytes.saturating_add(ALLOCATION_HEADER_BYTES),
        ALLOCATION_GRANULARITY,
    )
}

pub(crate) fn approximate_allocation_overhead(payload_bytes: usize) -> usize {
    approximate_allocation_size(payload_bytes).saturating_sub(payload_bytes)
}

fn round_up(value: usize, alignment: usize) -> usize {
    value.saturating_add(alignment.saturating_sub(1)) / alignment * alignment
}

/// One memory contribution attached to the current recorder path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub path: String,
    pub type_name: String,
    pub kind: MemoryRecordKind,
    pub bytes: usize,
}

/// Coarse explanation of where retained bytes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryRecordKind {
    /// Inline bytes of the measured value itself.
    Shallow,
    /// Initialized bytes in a heap allocation owned by a pointer/container.
    Heap,
    /// Allocated but currently unused container capacity.
    SpareCapacity,
    /// Best-effort accounting for layouts hidden by upstream crates or std.
    Approximate,
}

impl MemoryRecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryRecordKind::Shallow => "shallow",
            MemoryRecordKind::Heap => "heap",
            MemoryRecordKind::SpareCapacity => "spare capacity",
            MemoryRecordKind::Approximate => "approximate",
        }
    }
}

/// Controls whether the recorder also keeps every individual contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecorderMode {
    /// Keep only the total byte count.
    TotalOnly,
    /// Keep only totals for each `(path, type_name, kind)` bucket.
    Aggregate,
    /// Keep aggregated totals plus the raw contribution stream.
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MemoryRecordKey {
    path: String,
    type_name: String,
    kind: MemoryRecordKind,
}

/// Accumulates tagged memory records while preserving a logical path.
///
/// Recording is aggregated by default because project-wide profiles can emit hundreds of thousands
/// of contributions. Detailed mode is available for debugging recorder implementations, but normal
/// reports should depend on the grouped totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecorder {
    mode: MemoryRecorderMode,
    path: Vec<String>,
    records: BTreeMap<MemoryRecordKey, usize>,
    raw_records: Option<Vec<MemoryRecord>>,
    total_bytes: usize,
}

impl MemoryRecorder {
    pub fn new(root: impl Into<String>) -> Self {
        Self::with_mode(root, MemoryRecorderMode::Aggregate)
    }

    pub fn detailed(root: impl Into<String>) -> Self {
        Self::with_mode(root, MemoryRecorderMode::Detailed)
    }

    pub fn total_only(root: impl Into<String>) -> Self {
        Self::with_mode(root, MemoryRecorderMode::TotalOnly)
    }

    pub fn with_mode(root: impl Into<String>, mode: MemoryRecorderMode) -> Self {
        Self {
            mode,
            path: vec![root.into()],
            records: BTreeMap::new(),
            raw_records: match mode {
                MemoryRecorderMode::TotalOnly | MemoryRecorderMode::Aggregate => None,
                MemoryRecorderMode::Detailed => Some(Vec::new()),
            },
            total_bytes: 0,
        }
    }

    pub fn scope<R>(&mut self, label: impl Into<String>, f: impl FnOnce(&mut Self) -> R) -> R {
        if self.mode == MemoryRecorderMode::TotalOnly {
            return f(self);
        }

        self.path.push(label.into());
        let result = f(self);
        self.path.pop();
        result
    }

    pub fn record_shallow<T>(&mut self, bytes: usize)
    where
        T: ?Sized,
    {
        self.record::<T>(MemoryRecordKind::Shallow, bytes);
    }

    pub fn record_heap<T>(&mut self, bytes: usize)
    where
        T: ?Sized,
    {
        self.record::<T>(MemoryRecordKind::Heap, bytes);
    }

    pub fn record_spare_capacity<T>(&mut self, bytes: usize)
    where
        T: ?Sized,
    {
        self.record::<T>(MemoryRecordKind::SpareCapacity, bytes);
    }

    pub fn record_approximate<T>(&mut self, bytes: usize)
    where
        T: ?Sized,
    {
        self.record::<T>(MemoryRecordKind::Approximate, bytes);
    }

    pub fn record<T>(&mut self, kind: MemoryRecordKind, bytes: usize)
    where
        T: ?Sized,
    {
        self.record_type_name(kind, any::type_name::<T>(), bytes);
    }

    pub fn record_type_name(
        &mut self,
        kind: MemoryRecordKind,
        type_name: impl Into<String>,
        bytes: usize,
    ) {
        if bytes == 0 {
            return;
        }

        self.total_bytes = self.total_bytes.saturating_add(bytes);
        if self.mode == MemoryRecorderMode::TotalOnly {
            return;
        }

        let path = self.path.join(".");
        let type_name = type_name.into();
        let key = MemoryRecordKey {
            path: path.clone(),
            type_name: type_name.clone(),
            kind,
        };
        *self.records.entry(key).or_default() += bytes;

        if let Some(raw_records) = &mut self.raw_records {
            raw_records.push(MemoryRecord {
                path,
                type_name,
                kind,
                bytes,
            });
        }
    }

    pub fn mode(&self) -> MemoryRecorderMode {
        self.mode
    }

    pub fn records(&self) -> Vec<MemoryRecord> {
        self.records
            .iter()
            .map(|(key, bytes)| MemoryRecord {
                path: key.path.clone(),
                type_name: key.type_name.clone(),
                kind: key.kind,
                bytes: *bytes,
            })
            .collect()
    }

    pub fn raw_records(&self) -> Option<&[MemoryRecord]> {
        self.raw_records.as_deref()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn totals_by_path(&self) -> BTreeMap<&str, usize> {
        let mut totals = BTreeMap::new();
        for (key, bytes) in &self.records {
            *totals.entry(key.path.as_str()).or_default() += bytes;
        }
        totals
    }

    pub fn totals_by_kind(&self) -> BTreeMap<MemoryRecordKind, usize> {
        let mut totals = BTreeMap::new();
        for (key, bytes) in &self.records {
            *totals.entry(key.kind).or_default() += bytes;
        }
        totals
    }

    pub fn totals_by_type(&self) -> BTreeMap<&str, usize> {
        let mut totals = BTreeMap::new();
        for (key, bytes) in &self.records {
            *totals.entry(key.type_name.as_str()).or_default() += bytes;
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use std::{any, collections::BTreeMap};

    use super::{MemoryRecordKind, MemoryRecorder, MemoryRecorderMode};

    #[test]
    fn recorder_keeps_scoped_paths_and_totals() {
        let mut recorder = MemoryRecorder::new("project");
        recorder.scope("parse", |recorder| {
            recorder.record_heap::<String>(40);
            recorder.scope("files", |recorder| recorder.record_shallow::<usize>(2));
        });
        recorder.scope("body_ir", |recorder| {
            recorder.record_spare_capacity::<Vec<u8>>(8)
        });

        let totals = recorder.totals_by_path();
        assert_eq!(totals.get("project.parse"), Some(&40));
        assert_eq!(totals.get("project.parse.files"), Some(&2));
        assert_eq!(totals.get("project.body_ir"), Some(&8));
        assert_eq!(recorder.total_bytes(), 50);
    }

    #[test]
    fn recorder_summarizes_by_kind() {
        let mut recorder = MemoryRecorder::new("root");
        recorder.record_shallow::<usize>(3);
        recorder.record_heap::<String>(5);
        recorder.record_heap::<Vec<u8>>(7);

        let mut expected = BTreeMap::new();
        expected.insert(MemoryRecordKind::Shallow, 3);
        expected.insert(MemoryRecordKind::Heap, 12);

        assert_eq!(recorder.totals_by_kind(), expected);
    }

    #[test]
    fn recorder_attaches_type_names_to_records() {
        let mut recorder = MemoryRecorder::new("root");
        recorder.record_shallow::<usize>(8);
        recorder.record_heap::<String>(13);

        let records = recorder.records();
        assert!(
            records
                .iter()
                .any(|record| record.type_name == any::type_name::<usize>())
        );
        assert!(
            records
                .iter()
                .any(|record| record.type_name == any::type_name::<String>())
        );

        let totals = recorder.totals_by_type();
        assert_eq!(totals.get(any::type_name::<usize>()), Some(&8));
        assert_eq!(totals.get(any::type_name::<String>()), Some(&13));
    }

    #[test]
    fn recorder_can_attach_custom_type_names() {
        let mut recorder = MemoryRecorder::new("root");
        recorder.record_type_name(MemoryRecordKind::Approximate, "example::TokenData", 21);

        let records = recorder.records();
        let record = &records[0];
        assert_eq!(record.type_name, "example::TokenData");
        assert_eq!(record.bytes, 21);
    }

    #[test]
    fn recorder_aggregates_duplicate_contributions_by_default() {
        let mut recorder = MemoryRecorder::new("root");
        recorder.record_heap::<String>(5);
        recorder.record_heap::<String>(7);

        let records = recorder.records();
        assert_eq!(recorder.mode(), MemoryRecorderMode::Aggregate);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].bytes, 12);
        assert_eq!(recorder.raw_records(), None);
    }

    #[test]
    fn detailed_recorder_keeps_raw_contributions() {
        let mut recorder = MemoryRecorder::detailed("root");
        recorder.record_heap::<String>(5);
        recorder.record_heap::<String>(7);

        let records = recorder.records();
        let raw_records = recorder
            .raw_records()
            .expect("detailed recorder should keep raw records");

        assert_eq!(recorder.mode(), MemoryRecorderMode::Detailed);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].bytes, 12);
        assert_eq!(raw_records.len(), 2);
        assert_eq!(raw_records[0].bytes, 5);
        assert_eq!(raw_records[1].bytes, 7);
    }

    #[test]
    fn total_only_recorder_keeps_totals_without_records() {
        let mut recorder = MemoryRecorder::total_only("root");
        recorder.scope("ignored", |recorder| {
            recorder.record_heap::<String>(5);
            recorder.record_heap::<Vec<u8>>(7);
        });

        assert_eq!(recorder.mode(), MemoryRecorderMode::TotalOnly);
        assert_eq!(recorder.total_bytes(), 12);
        assert!(recorder.records().is_empty());
        assert!(recorder.totals_by_path().is_empty());
    }
}
