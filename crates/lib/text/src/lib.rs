//! Interned short text used by rust-glancer's semantic indexes.
//!
//! `Name` deliberately keeps rendering and comparison cheap without requiring query callers to
//! carry an interner. The interner is a reuse table; cloned `Name`s retain the shared string
//! allocation through `Arc<str>`, while the interner itself can prune names that no live analysis
//! snapshot still references.

use std::{
    borrow::Borrow,
    collections::{HashMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash as _, Hasher as _},
    ops::Deref,
    sync::{Arc, Weak},
};

/// Shared short text, usually an identifier or path segment.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Arc<str>);

impl Name {
    /// Builds a standalone name without looking it up in an interner.
    ///
    /// Production lowering should prefer `NameInterner::intern`; this constructor keeps tests and
    /// small synthetic query values lightweight.
    pub fn new(text: impl AsRef<str>) -> Self {
        Self(Arc::from(text.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn shrink_to_fit(&mut self) {}
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self(Arc::from(value.into_boxed_str()))
    }
}

// Archive names as plain strings. That keeps archived map keys hashable and avoids making the
// runtime interner part of the cache format.
impl rkyv::Archive for Name {
    type Archived = rkyv::string::ArchivedString;
    type Resolver = rkyv::string::StringResolver;

    fn resolve(&self, resolver: Self::Resolver, out: rkyv::Place<Self::Archived>) {
        rkyv::string::ArchivedString::resolve_from_str(self.as_str(), resolver, out);
    }
}

impl<S> rkyv::Serialize<S> for Name
where
    S: rkyv::rancor::Fallible + ?Sized,
    S::Error: rkyv::rancor::Source,
    str: rkyv::SerializeUnsized<S>,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        rkyv::string::ArchivedString::serialize_from_str(self.as_str(), serializer)
    }
}

impl<D> rkyv::Deserialize<Name, D> for rkyv::string::ArchivedString
where
    D: rkyv::rancor::Fallible + ?Sized,
{
    fn deserialize(&self, _: &mut D) -> Result<Name, D::Error> {
        Ok(Name::new(self.as_str()))
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Reuse table that deduplicates short text allocations without owning them forever.
///
/// The table stores weak handles grouped by text hash. Phase data owns the strong `Name`s; once a
/// rebuild drops obsolete phase data, `shrink_to_fit` removes the now-dead weak handles.
#[derive(Debug, Clone, Default)]
pub struct NameInterner {
    buckets: HashMap<u64, Vec<Weak<str>>>,
}

/// Independent name reuse tables keyed by package slot.
///
/// Package-level interners preserve the cheap `Name` handles while avoiding a single mutable
/// interner that would serialize package-level lowering. Equal names still compare by text, so
/// sharing allocations across package boundaries is an optimization, not a correctness property.
#[derive(Debug, Clone, Default)]
pub struct PackageNameInterners {
    packages: Vec<NameInterner>,
}

impl NameInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, text: impl AsRef<str>) -> Name {
        let text = text.as_ref();
        let hash = Self::hash_text(text);

        if let Some(bucket) = self.buckets.get_mut(&hash) {
            let mut index = 0;
            while index < bucket.len() {
                match bucket[index].upgrade() {
                    Some(name) if name.as_ref() == text => return Name(name),
                    Some(_) => index += 1,
                    None => {
                        bucket.swap_remove(index);
                    }
                }
            }
        }

        let name = Name::new(text);
        self.buckets
            .entry(hash)
            .or_default()
            .push(Arc::downgrade(&name.0));
        name
    }

    pub fn len(&self) -> usize {
        self.buckets
            .values()
            .map(|bucket| bucket.iter().filter(|name| name.strong_count() > 0).count())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets
            .values()
            .all(|bucket| bucket.iter().all(|name| name.strong_count() == 0))
    }

    pub fn shrink_to_fit(&mut self) {
        self.buckets.retain(|_, bucket| {
            bucket.retain(|name| name.strong_count() > 0);
            bucket.shrink_to_fit();
            !bucket.is_empty()
        });
        self.buckets.shrink_to_fit();
    }

    fn hash_text(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

impl PackageNameInterners {
    pub fn new(package_count: usize) -> Self {
        let mut packages = Vec::with_capacity(package_count);
        packages.resize_with(package_count, NameInterner::new);
        Self { packages }
    }

    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    pub fn package_mut(&mut self, package_slot: usize) -> Option<&mut NameInterner> {
        self.packages.get_mut(package_slot)
    }

    /// Returns package-local interners as disjoint mutable slots for package-parallel lowering.
    pub fn packages_mut(&mut self) -> &mut [NameInterner] {
        &mut self.packages
    }

    pub fn shrink_to_fit(&mut self) {
        self.packages.shrink_to_fit();
        for package in &mut self.packages {
            package.shrink_to_fit();
        }
    }

    pub fn len(&self) -> usize {
        self.packages.iter().map(NameInterner::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.iter().all(NameInterner::is_empty)
    }
}

#[cfg(feature = "memsize")]
mod memsize {
    use std::{mem, sync::Weak};

    use rg_memsize::{MemoryRecorder, MemorySize};

    use crate::{Name, NameInterner, PackageNameInterners};

    impl MemorySize for Name {
        fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
    }

    impl MemorySize for NameInterner {
        fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
            recorder.scope("buckets", |recorder| {
                let entry_payload = mem::size_of::<u64>() + mem::size_of::<Vec<Weak<str>>>();
                recorder
                    .record_heap::<u64>(self.buckets.len().saturating_mul(mem::size_of::<u64>()));
                recorder.record_heap::<Vec<Weak<str>>>(
                    self.buckets
                        .len()
                        .saturating_mul(mem::size_of::<Vec<Weak<str>>>()),
                );
                recorder.record_spare_capacity::<NameInterner>(
                    self.buckets
                        .capacity()
                        .saturating_sub(self.buckets.len())
                        .saturating_mul(entry_payload),
                );
            });

            recorder.scope("weak_entries", |recorder| {
                let len = self.buckets.values().map(Vec::len).sum::<usize>();
                let capacity = self.buckets.values().map(Vec::capacity).sum::<usize>();
                recorder.record_heap::<Weak<str>>(len.saturating_mul(mem::size_of::<Weak<str>>()));
                recorder.record_spare_capacity::<Weak<str>>(
                    capacity
                        .saturating_sub(len)
                        .saturating_mul(mem::size_of::<Weak<str>>()),
                );
            });

            recorder.scope("live_text", |recorder| {
                let mut live_count = 0usize;
                for name in self
                    .buckets
                    .values()
                    .flat_map(|bucket| bucket.iter())
                    .filter_map(Weak::upgrade)
                {
                    live_count += 1;
                    recorder.record_heap::<str>(name.len());
                }

                // Arc's ref-count header lives with the string allocation, but `Name` itself is a
                // cheap handle and deliberately does not record it. Counting it here keeps interned
                // text attributed once, next to the reuse table that can enumerate live names.
                recorder.record_approximate::<Name>(
                    live_count.saturating_mul(mem::size_of::<usize>() * 2),
                );
            });
        }
    }

    impl MemorySize for PackageNameInterners {
        fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
            recorder.scope("packages", |recorder| {
                recorder.record_heap::<NameInterner>(
                    self.packages
                        .len()
                        .saturating_mul(mem::size_of::<NameInterner>()),
                );
                recorder.record_spare_capacity::<NameInterner>(
                    self.packages
                        .capacity()
                        .saturating_sub(self.packages.len())
                        .saturating_mul(mem::size_of::<NameInterner>()),
                );
            });

            for (package_slot, package) in self.packages.iter().enumerate() {
                recorder.scope(format!("package_{package_slot}"), |recorder| {
                    package.record_memory_children(recorder);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Name, NameInterner, PackageNameInterners};

    #[test]
    fn interner_reuses_existing_names() {
        let mut interner = NameInterner::new();

        let first = interner.intern("User");
        let second = interner.intern("User");

        assert_eq!(first, second);
        assert_eq!(first.as_str().as_ptr(), second.as_str().as_ptr());
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn interner_prunes_names_that_no_live_data_owns() {
        let mut interner = NameInterner::new();

        let name = interner.intern("Temporary");
        assert_eq!(interner.len(), 1);
        assert_eq!(stored_weak_count(&interner), 1);

        drop(name);
        assert_eq!(interner.len(), 0);
        assert_eq!(stored_weak_count(&interner), 1);

        interner.shrink_to_fit();
        assert_eq!(interner.len(), 0);
        assert_eq!(stored_weak_count(&interner), 0);
        assert!(interner.is_empty());
    }

    #[test]
    fn interner_reuses_live_name_after_pruning_dead_neighbors() {
        let mut interner = NameInterner::new();

        let live = interner.intern("User");
        let stale = interner.intern("Thing");
        drop(stale);

        interner.shrink_to_fit();
        let reused = interner.intern("User");

        assert_eq!(live.as_str().as_ptr(), reused.as_str().as_ptr());
        assert_eq!(interner.len(), 1);
        assert_eq!(stored_weak_count(&interner), 1);
    }

    #[test]
    fn names_compare_and_render_like_strings() {
        let name = Name::new("User");

        assert_eq!(name, "User");
        assert_eq!(name.as_str(), "User");
        assert_eq!(name.to_string(), "User");
        assert_eq!(format!("{name:?}"), "\"User\"");
    }

    #[test]
    fn package_interners_keep_allocations_package_local() {
        let mut interners = PackageNameInterners::new(2);

        let first = interners
            .package_mut(0)
            .expect("package zero interner should exist")
            .intern("User");
        let second = interners
            .package_mut(1)
            .expect("package one interner should exist")
            .intern("User");

        assert_eq!(first, second);
        assert_ne!(first.as_str().as_ptr(), second.as_str().as_ptr());
        assert_eq!(interners.len(), 2);
    }

    #[cfg(feature = "memsize")]
    #[test]
    fn interner_records_unique_text_payload() {
        use rg_memsize::{MemoryRecordKind, MemoryRecorder, MemorySize};

        let mut interner = NameInterner::new();
        let user = interner.intern("User");
        let duplicate = interner.intern("User");
        let thing = interner.intern("Thing");

        assert_eq!(user.as_str().as_ptr(), duplicate.as_str().as_ptr());
        assert_eq!(interner.len(), 2);

        let mut recorder = MemoryRecorder::new("names");
        interner.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert!(
            totals
                .get(&MemoryRecordKind::Heap)
                .is_some_and(|bytes| *bytes >= "UserThing".len())
        );

        drop((user, duplicate, thing));
    }

    fn stored_weak_count(interner: &NameInterner) -> usize {
        interner.buckets.values().map(Vec::len).sum()
    }
}
