//! Package-slot-indexed storage for retained analysis package data.
//!
//! Package payloads are retained behind `Arc` while resident, and selected slots can be marked as
//! offloaded after a durable package artifact is written by the project cache layer. Read
//! transactions can receive a loader for offloaded slots, so callers still work with logical
//! package slots instead of treating residency as project topology. A phase may also retain a
//! compact summary inside an offloaded slot when a query needs metadata without loading the full
//! payload.

mod error;
mod loader;
mod txn;

use std::sync::Arc;

use rg_std::{MemoryRecorder, MemorySize, Shrink};
use rg_workspace::PackageSlot;

pub use self::{
    error::{MalformedCacheError, PackageLoadError, PackageStoreError},
    loader::{LoadPackage, PackageLoader},
    txn::PackageStoreReadTxn,
};

/// Package slots visible inside one read transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSubset {
    packages: Vec<bool>,
}

impl PackageSubset {
    /// Includes every slot in a package-store snapshot.
    pub fn all(package_count: usize) -> Self {
        Self {
            packages: vec![true; package_count],
        }
    }

    /// Starts with every slot excluded so callers can add the logical view they need.
    pub fn empty(package_count: usize) -> Self {
        Self {
            packages: vec![false; package_count],
        }
    }

    pub fn raw_len(&self) -> usize {
        self.packages.len()
    }

    pub fn contains(&self, package: PackageSlot) -> bool {
        self.packages.get(package.0).copied().unwrap_or(false)
    }

    pub fn insert(&mut self, package: PackageSlot) -> bool {
        let Some(slot) = self.packages.get_mut(package.0) else {
            return false;
        };
        let was_absent = !*slot;
        *slot = true;
        was_absent
    }
}

/// Package storage keyed by the stable package slots of one workspace snapshot.
///
/// `OffloadedState` defaults to `()` for phases that drop their entire payload. A phase with useful
/// compact state can provide another type, which is then stored directly in the offloaded variant
/// instead of in a separately synchronized side table.
// Dev note: we intentionally do not expose convenience methods like `resident_packages`,
// since they would give an interface over `&T` or `&mut T`, they are prone for hard-to-find
// bugs; instead, we expose verbose APIs to force caller to think about the state of the
// package entry.
// tl;dr: we don't want to make an illusion of "here are all the packages" while returning
// _not_ all the packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageStore<T, OffloadedState = ()> {
    packages: Vec<PackageEntry<T, OffloadedState>>,
}

// An empty store does not need an empty payload or offloaded-state value. Keeping this impl
// unconditional lets phase-specific metadata omit `Default` when an empty summary would be invalid.
impl<T, OffloadedState> Default for PackageStore<T, OffloadedState> {
    fn default() -> Self {
        Self {
            packages: Vec::new(),
        }
    }
}

impl<T> PackageStore<T> {
    /// Creates the package-shaped baseline used before selected source packages are built.
    pub fn all_offloaded(package_count: usize) -> Self {
        Self::from_entries(
            (0..package_count)
                .map(|_| PackageEntry::offloaded())
                .collect(),
        )
    }
}

impl<T, OffloadedState> PackageStore<T, OffloadedState> {
    /// Builds a store from explicit resident/offloaded package entries.
    ///
    /// Fresh phase construction starts with offloaded entries and replaces source-built packages.
    /// Startup-cache loading can mix exact offloaded summaries with resident payloads while
    /// preserving the same package-slot shape.
    pub fn from_entries(packages: Vec<PackageEntry<T, OffloadedState>>) -> Self {
        Self { packages }
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Returns one raw package storage entry by package slot.
    pub fn raw_entry(&self, package: PackageSlot) -> Option<&PackageEntry<T, OffloadedState>> {
        self.packages.get(package.0)
    }

    /// Iterates over all raw package storage entries, including offloaded slots.
    pub fn raw_entries(&self) -> impl Iterator<Item = &PackageEntry<T, OffloadedState>> + '_ {
        self.packages.iter()
    }

    /// Iterates over all raw package storage entries together with their original package slots.
    pub fn raw_entries_with_slots(
        &self,
    ) -> impl Iterator<Item = (PackageSlot, &PackageEntry<T, OffloadedState>)> {
        self.packages
            .iter()
            .enumerate()
            .map(|(package_idx, entry)| (PackageSlot(package_idx), entry))
    }

    /// Builds a logical read transaction over every package slot.
    ///
    /// Resident packages are available immediately. Offloaded packages are represented by lazy
    /// entries and loaded through the injected loader only if a query touches that slot.
    pub fn read_txn<'db>(&'db self, loader: PackageLoader<'db, T>) -> PackageStoreReadTxn<'db, T> {
        PackageStoreReadTxn::from_store_entries(
            self.packages
                .iter()
                .map(|entry| (true, entry.resident_arc())),
            loader,
        )
    }

    /// Builds a logical read transaction over selected package slots.
    ///
    /// Excluded packages remain present as logical slots, but direct reads fail with an explicit
    /// subset error while broad materialization helpers skip them.
    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: PackageLoader<'db, T>,
        subset: &PackageSubset,
    ) -> PackageStoreReadTxn<'db, T> {
        debug_assert_eq!(
            subset.raw_len(),
            self.packages.len(),
            "package subset should belong to the same package-store snapshot",
        );

        PackageStoreReadTxn::from_store_entries(
            self.packages
                .iter()
                .enumerate()
                .map(|(package_idx, entry)| {
                    let package = PackageSlot(package_idx);
                    let resident_package = entry.resident_arc();
                    (subset.contains(package), resident_package)
                }),
            loader,
        )
    }

    /// Replaces one package payload while preserving all other cloned snapshot entries.
    pub fn replace(&mut self, package: PackageSlot, value: T) -> Option<()> {
        let slot = self.packages.get_mut(package.0)?;
        *slot = PackageEntry {
            state: PackageEntryState::Resident(Arc::new(value)),
        };
        Some(())
    }

    /// Drops one resident payload after a durable package artifact has been written.
    pub fn offload(&mut self, package: PackageSlot) -> Option<()>
    where
        OffloadedState: Default,
    {
        self.offload_with(package, OffloadedState::default())
    }

    /// Drops one resident payload while retaining a small phase-specific summary in its slot.
    pub fn offload_with(&mut self, package: PackageSlot, state: OffloadedState) -> Option<()> {
        let slot = self.packages.get_mut(package.0)?;
        *slot = PackageEntry::offloaded_with(state);
        Some(())
    }

    /// Returns mutable access only when this snapshot uniquely owns the package payload.
    pub fn get_unique_mut(&mut self, package: PackageSlot) -> Option<&mut T> {
        self.packages.get_mut(package.0)?.as_resident_unique_mut()
    }

    /// Returns mutable access, cloning the package payload if another snapshot still shares it.
    pub fn make_mut(&mut self, package: PackageSlot) -> Option<&mut T>
    where
        T: Clone,
    {
        self.packages.get_mut(package.0)?.make_mut()
    }
}

/// Retained storage state for one package slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry<T, OffloadedState = ()> {
    state: PackageEntryState<T, OffloadedState>,
}

/// Internal representation for one package-store entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PackageEntryState<T, OffloadedState> {
    Resident(Arc<T>),
    Offloaded(OffloadedState),
}

impl<T> PackageEntry<T> {
    /// Creates a lazy package slot that retains no state beyond its package identity.
    pub fn offloaded() -> Self {
        Self::offloaded_with(())
    }
}

impl<T, OffloadedState> PackageEntry<T, OffloadedState> {
    /// Creates an immediately available package payload.
    pub fn resident(package: T) -> Self {
        Self {
            state: PackageEntryState::Resident(Arc::new(package)),
        }
    }

    /// Creates a lazy package slot with a small summary that remains directly queryable.
    pub fn offloaded_with(state: OffloadedState) -> Self {
        Self {
            state: PackageEntryState::Offloaded(state),
        }
    }

    /// Returns the resident package payload, if this slot is currently in memory.
    pub fn as_resident(&self) -> Option<&T> {
        match &self.state {
            PackageEntryState::Resident(package) => Some(package.as_ref()),
            PackageEntryState::Offloaded(_) => None,
        }
    }

    /// Clones the retained handle for a phase-specific read transaction.
    pub fn resident_arc(&self) -> Option<Arc<T>> {
        match &self.state {
            PackageEntryState::Resident(package) => Some(Arc::clone(package)),
            PackageEntryState::Offloaded(_) => None,
        }
    }

    /// Returns the compact state retained in place of an offloaded package payload.
    pub fn as_offloaded(&self) -> Option<&OffloadedState> {
        match &self.state {
            PackageEntryState::Resident(_) => None,
            PackageEntryState::Offloaded(state) => Some(state),
        }
    }

    /// Returns whether this slot has been intentionally dropped from resident memory.
    pub fn is_offloaded(&self) -> bool {
        matches!(self.state, PackageEntryState::Offloaded(_))
    }

    /// Returns unique mutable access to the resident payload, if no cloned snapshot shares it.
    pub fn as_resident_unique_mut(&mut self) -> Option<&mut T> {
        match &mut self.state {
            PackageEntryState::Resident(package) => Arc::get_mut(package),
            PackageEntryState::Offloaded(_) => None,
        }
    }

    fn make_mut(&mut self) -> Option<&mut T>
    where
        T: Clone,
    {
        match &mut self.state {
            PackageEntryState::Resident(package) => Some(Arc::make_mut(package)),
            PackageEntryState::Offloaded(_) => None,
        }
    }
}

impl<T, OffloadedState> Shrink for PackageStore<T, OffloadedState>
where
    T: Shrink,
    OffloadedState: Shrink,
{
    fn shrink_to_fit(&mut self) {
        self.packages.shrink_to_fit();
        for entry in &mut self.packages {
            Shrink::shrink_to_fit(entry);
        }
    }
}

impl<T, OffloadedState> Shrink for PackageEntry<T, OffloadedState>
where
    T: Shrink,
    OffloadedState: Shrink,
{
    fn shrink_to_fit(&mut self) {
        // Resident payloads may be shared with older snapshots or read transactions. Compacting
        // only uniquely-owned payloads preserves copy-on-write sharing instead of cloning data
        // just to release spare capacity.
        match &mut self.state {
            PackageEntryState::Resident(package) => {
                if let Some(package) = Arc::get_mut(package) {
                    Shrink::shrink_to_fit(package);
                }
            }
            PackageEntryState::Offloaded(state) => Shrink::shrink_to_fit(state),
        }
    }
}

impl<T, OffloadedState> MemorySize for PackageStore<T, OffloadedState>
where
    T: MemorySize,
    OffloadedState: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.packages.record_memory_children(recorder);
    }
}

impl<T, OffloadedState> MemorySize for PackageEntry<T, OffloadedState>
where
    T: MemorySize,
    OffloadedState: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match &self.state {
            PackageEntryState::Resident(package) => package.record_memory_children(recorder),
            PackageEntryState::Offloaded(state) => state.record_memory_children(recorder),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rg_std::Shrink;
    use rg_workspace::PackageSlot;

    use crate::{
        LoadPackage, PackageEntry, PackageLoader, PackageStore, PackageStoreError, PackageSubset,
    };

    #[derive(Debug)]
    struct TestLoader {
        loads: AtomicUsize,
        packages: Vec<&'static str>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ShrinkProbe {
        calls: usize,
    }

    impl Shrink for ShrinkProbe {
        fn shrink_to_fit(&mut self) {
            self.calls += 1;
        }
    }

    impl LoadPackage<&'static str> for TestLoader {
        fn load(&self, slot: PackageSlot) -> Result<Arc<&'static str>, PackageStoreError> {
            self.loads.fetch_add(1, Ordering::Relaxed);
            let Some(package) = self.packages.get(slot.0) else {
                return Err(PackageStoreError::MissingSlot { slot });
            };
            Ok(Arc::new(*package))
        }
    }

    fn resident_store<T>(packages: Vec<T>) -> PackageStore<T> {
        PackageStore::from_entries(packages.into_iter().map(PackageEntry::resident).collect())
    }

    #[test]
    fn cloned_stores_replace_packages_independently() {
        let original = resident_store(vec!["workspace", "dependency"]);
        let mut changed = original.clone();

        changed
            .replace(PackageSlot(1), "rebuilt")
            .expect("package slot should exist");

        let original_residents = original
            .raw_entries_with_slots()
            .filter_map(|(slot, entry)| entry.as_resident().map(|package| (slot.0, *package)))
            .collect::<Vec<_>>();
        let changed_residents = changed
            .raw_entries_with_slots()
            .filter_map(|(slot, entry)| entry.as_resident().map(|package| (slot.0, *package)))
            .collect::<Vec<_>>();

        assert_eq!(
            original_residents,
            vec![(0, "workspace"), (1, "dependency")]
        );
        assert_eq!(changed_residents, vec![(0, "workspace"), (1, "rebuilt")]);
    }

    #[test]
    fn offloaded_entries_keep_phase_specific_state() {
        let mut store: PackageStore<&str, &str> =
            PackageStore::from_entries(vec![PackageEntry::resident("resident bodies")]);
        store
            .offload_with(PackageSlot(0), "complete coverage")
            .expect("package slot should exist");

        assert_eq!(
            store
                .raw_entry(PackageSlot(0))
                .and_then(PackageEntry::as_offloaded),
            Some(&"complete coverage"),
        );

        store
            .replace(PackageSlot(0), "resident bodies")
            .expect("package slot should exist");
        let resident = store
            .raw_entry(PackageSlot(0))
            .expect("package slot should exist");
        assert_eq!(resident.as_resident(), Some(&"resident bodies"));
        assert!(resident.as_offloaded().is_none());
    }

    #[test]
    fn subset_read_transactions_preserve_original_package_slots() {
        let store = resident_store(vec!["workspace", "hidden", "dependency"]);
        let loader = Arc::new(TestLoader {
            loads: AtomicUsize::new(0),
            packages: vec!["workspace", "hidden", "dependency"],
        });
        let mut subset = PackageSubset::empty(store.len());
        subset.insert(PackageSlot(0));
        subset.insert(PackageSlot(2));
        let txn = store.read_txn_for_subset(PackageLoader::from_arc(loader), &subset);

        let included_packages = txn
            .included_packages()
            .map(|p| p.expect("Must exist"))
            .collect::<Vec<_>>();

        assert_eq!(txn.read(PackageSlot(0)).unwrap(), included_packages[0]);
        assert!(matches!(
            txn.read(PackageSlot(1)),
            Err(PackageStoreError::ExcludedSlot {
                slot: PackageSlot(1)
            }),
        ));
        assert_eq!(txn.read(PackageSlot(2)).unwrap(), included_packages[1]);
    }

    #[test]
    fn offloaded_packages_are_not_resident_until_materialized() {
        let mut store = resident_store(vec!["workspace", "dependency"]);

        store
            .offload(PackageSlot(1))
            .expect("package slot should exist");

        let residents = store
            .raw_entries_with_slots()
            .filter_map(|(slot, entry)| entry.as_resident().map(|package| (slot.0, *package)))
            .collect::<Vec<_>>();

        assert_eq!(residents, vec![(0, "workspace")]);
        assert!(
            store
                .raw_entry(PackageSlot(1))
                .expect("offloaded package slot should exist")
                .is_offloaded()
        );
    }

    #[test]
    fn shrink_compacts_unique_resident_packages_without_cloning_shared_ones() {
        let original = PackageStore::from_entries(vec![
            PackageEntry::resident(ShrinkProbe { calls: 0 }),
            PackageEntry::resident(ShrinkProbe { calls: 0 }),
            PackageEntry::offloaded(),
        ]);
        let mut cloned = original.clone();

        cloned
            .replace(PackageSlot(1), ShrinkProbe { calls: 0 })
            .expect("package slot should exist");
        Shrink::shrink_to_fit(&mut cloned);

        let calls = cloned
            .raw_entries_with_slots()
            .map(|(slot, entry)| (slot.0, entry.as_resident().map(|probe| probe.calls)))
            .collect::<Vec<_>>();

        assert_eq!(calls, vec![(0, Some(0)), (1, Some(1)), (2, None)]);
    }

    #[test]
    fn memory_accounting_includes_slot_storage_and_resident_payloads() {
        use std::mem;

        use rg_std::MemorySize;

        let offloaded = PackageStore::<String>::from_entries(vec![
            PackageEntry::offloaded(),
            PackageEntry::offloaded(),
        ]);
        let resident = PackageStore::from_entries(vec![
            PackageEntry::offloaded(),
            PackageEntry::resident("user".to_string()),
        ]);

        assert!(
            offloaded.memory_size() > mem::size_of::<PackageStore<String>>(),
            "offloaded stores should still count package-slot vector storage",
        );
        assert!(
            resident.memory_size() > offloaded.memory_size(),
            "resident packages should add their Arc-backed payload accounting",
        );
    }

    #[test]
    fn read_transactions_load_offloaded_packages_lazily() {
        let mut store = resident_store(vec!["workspace", "dependency"]);
        store
            .offload(PackageSlot(1))
            .expect("package slot should exist");

        let loader = Arc::new(TestLoader {
            loads: AtomicUsize::new(0),
            packages: vec!["workspace", "dependency"],
        });
        let txn = store.read_txn(PackageLoader::from_arc(loader.clone()));

        assert_eq!(loader.loads.load(Ordering::Relaxed), 0);
        assert_eq!(
            *txn.read(PackageSlot(0))
                .expect("resident package should be readable"),
            "workspace",
        );
        assert_eq!(loader.loads.load(Ordering::Relaxed), 0);

        assert_eq!(
            *txn.read(PackageSlot(1))
                .expect("offloaded package should be loaded"),
            "dependency",
        );
        assert_eq!(
            *txn.read(PackageSlot(1))
                .expect("offloaded package should stay cached"),
            "dependency",
        );
        assert_eq!(loader.loads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn explicit_entries_can_start_with_offloaded_slots() {
        let store = PackageStore::from_entries(vec![
            PackageEntry::resident("workspace"),
            PackageEntry::offloaded(),
            PackageEntry::resident("local"),
        ]);

        let entries = store
            .raw_entries_with_slots()
            .map(|(slot, entry)| (slot.0, entry.as_resident().copied(), entry.is_offloaded()))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            vec![
                (0, Some("workspace"), false),
                (1, None, true),
                (2, Some("local"), false),
            ],
        );
    }

    #[test]
    fn subset_read_transactions_exclude_out_of_subset_packages() {
        let mut store = resident_store(vec!["workspace", "dependency", "unrelated"]);
        store
            .offload(PackageSlot(1))
            .expect("package slot should exist");

        let loader = Arc::new(TestLoader {
            loads: AtomicUsize::new(0),
            packages: vec!["workspace", "dependency", "unrelated"],
        });
        let mut subset = PackageSubset::empty(store.len());
        subset.insert(PackageSlot(0));
        subset.insert(PackageSlot(1));
        let txn = store.read_txn_for_subset(PackageLoader::from_arc(loader.clone()), &subset);

        assert_eq!(
            *txn.read(PackageSlot(1))
                .expect("included offloaded package should be loaded"),
            "dependency",
        );
        assert!(matches!(
            txn.read(PackageSlot(2)),
            Err(PackageStoreError::ExcludedSlot {
                slot: PackageSlot(2)
            }),
        ));
        assert_eq!(loader.loads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn raw_entries_preserve_original_package_slots() {
        let mut store = resident_store(vec!["workspace", "offloaded", "dependency"]);

        store
            .offload(PackageSlot(1))
            .expect("package slot should exist");

        let resident_entries = store
            .raw_entries()
            .filter_map(|entry| entry.as_resident().copied())
            .collect::<Vec<_>>();
        let raw_entries_with_slots = store
            .raw_entries_with_slots()
            .map(|(slot, entry)| (slot.0, entry.as_resident().copied(), entry.is_offloaded()))
            .collect::<Vec<_>>();

        assert_eq!(resident_entries, vec!["workspace", "dependency"]);
        assert_eq!(
            raw_entries_with_slots,
            vec![
                (0, Some("workspace"), false),
                (1, None, true),
                (2, Some("dependency"), false),
            ]
        );
    }
}
