//! Snapshot-scoped canonical crate declarations shared by independent solver sessions.
//!
//! A crate declaration is lowered from its own semantic origin, so the result does not depend on
//! the crate that later asks whether one of its impls is visible. Body IR resolves many use-site
//! crates in parallel; sharing these owned summaries avoids lowering the same dependency
//! declaration for every crate while each session keeps its own visible impl set and Chalk forests.

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use rg_ir_model::{FunctionRef, GenericDefRef, ImplRef, TraitDefRef, TypeAliasRef};

use crate::signature::TraitHeader;
use crate::{CallableSignature, ImplHeader, OpaqueTy, TraitRefLowering, Ty};

/// Opaque identities declared by one owner together with each identity's lowered bounds.
pub(super) type OpaqueBounds = Vec<(OpaqueTy, Vec<TraitRefLowering>)>;

/// One cache entry whose loader can fail without poisoning later requests.
///
/// The `OnceLock` stores both a found declaration and a stable absence. The small mutex only
/// serializes the first successful load for this key; initialized reads do not take it.
struct DeclarationSlot<V> {
    value: OnceLock<Option<Arc<V>>>,
    load: Mutex<()>,
}

impl<V> Default for DeclarationSlot<V> {
    fn default() -> Self {
        Self {
            value: OnceLock::new(),
            load: Mutex::new(()),
        }
    }
}

/// Canonical crate declarations shared while one immutable semantic snapshot is being analyzed.
///
/// The cache contains no visibility decisions, inference values, or solver answers. Its owner
/// creates it at a semantic-snapshot boundary and clones the handle into every use-site session
/// built from that snapshot. Body-origin declarations do not enter this cache because their path
/// resolver may expose request-local items. A later snapshot must get a new cache.
#[derive(Clone, Default)]
pub struct TraitSelectionDeclarationCache {
    shared: Arc<TraitSelectionDeclarations>,
}

/// Shared storage released after the last build or query owner drops its cache handle.
#[derive(Default)]
struct TraitSelectionDeclarations {
    impl_headers: DeclarationMap<ImplRef, ImplHeader>,
    trait_headers: DeclarationMap<TraitDefRef, TraitHeader>,
    type_alias_tys: DeclarationMap<TypeAliasRef, Ty>,
    function_signatures: DeclarationMap<FunctionRef, CallableSignature>,
    opaque_bounds: DeclarationMap<GenericDefRef, OpaqueBounds>,
}

impl Drop for TraitSelectionDeclarations {
    fn drop(&mut self) {
        // Recording every lookup through the shared profiler would make profiling itself contend
        // on hot declarations. Fold relaxed atomic counters into the profile once the snapshot
        // cache dies.
        self.impl_headers
            .report("impl_header.hit", "impl_header.miss");
        self.trait_headers
            .report("trait_header.hit", "trait_header.miss");
        self.type_alias_tys
            .report("type_alias_ty.hit", "type_alias_ty.miss");
        self.function_signatures
            .report("function_signature.hit", "function_signature.miss");
        self.opaque_bounds
            .report("opaque_bounds.hit", "opaque_bounds.miss");
    }
}

/// Concurrent declaration table with a separate initialization slot for each identity.
///
/// The map lock protects only identity-to-slot lookup. Declaration lowering happens after that
/// lock is released, so unrelated missing declarations can be loaded in parallel.
struct DeclarationMap<K, V> {
    entries: RwLock<HashMap<K, Arc<DeclarationSlot<V>>>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<K, V> Default for DeclarationMap<K, V> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }
}

/// Whether this request published a declaration result or reused an initialized slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationCacheAccess {
    Hit,
    Miss,
}

impl TraitSelectionDeclarationCache {
    /// Start an empty declaration cache for one semantic snapshot.
    ///
    /// Cloning the returned handle shares its entries; calling `new` again starts an independent
    /// cache for another snapshot or standalone session.
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn impl_header<E>(
        &self,
        impl_ref: ImplRef,
        load: impl FnOnce() -> Result<Option<ImplHeader>, E>,
    ) -> Result<Option<Arc<ImplHeader>>, E> {
        let (header, _) = self.shared.impl_headers.get_or_try_init(impl_ref, load)?;
        Ok(header)
    }

    pub(super) fn trait_header<E>(
        &self,
        trait_ref: TraitDefRef,
        load: impl FnOnce() -> Result<Option<TraitHeader>, E>,
    ) -> Result<Option<Arc<TraitHeader>>, E> {
        let (header, _) = self.shared.trait_headers.get_or_try_init(trait_ref, load)?;
        Ok(header)
    }

    pub(super) fn type_alias_ty<E>(
        &self,
        alias: TypeAliasRef,
        load: impl FnOnce() -> Result<Option<Ty>, E>,
    ) -> Result<Option<Arc<Ty>>, E> {
        let (ty, _) = self.shared.type_alias_tys.get_or_try_init(alias, load)?;
        Ok(ty)
    }

    pub(super) fn function_signature<E>(
        &self,
        function: FunctionRef,
        load: impl FnOnce() -> Result<Option<CallableSignature>, E>,
    ) -> Result<Option<Arc<CallableSignature>>, E> {
        let (signature, _) = self
            .shared
            .function_signatures
            .get_or_try_init(function, load)?;
        Ok(signature)
    }

    pub(super) fn opaque_bounds<E>(
        &self,
        owner: GenericDefRef,
        load: impl FnOnce() -> Result<OpaqueBounds, E>,
    ) -> Result<Arc<OpaqueBounds>, E> {
        let (bounds, _) = self
            .shared
            .opaque_bounds
            .get_or_try_init(owner, || load().map(Some))?;
        Ok(bounds.expect("opaque-bounds cache loader always stores a value"))
    }
}

impl<K, V> DeclarationMap<K, V>
where
    K: Eq + Hash,
{
    /// Load one declaration while holding only its per-key initialization lock.
    ///
    /// Initialized entries use a concurrent map read and a lock-free `OnceLock` read. Contenders
    /// for the same missing declaration wait for the first loader; errors leave the slot empty so a
    /// later request can retry rather than turning a transient package-load failure into cached
    /// semantic absence.
    fn get_or_try_init<E>(
        &self,
        key: K,
        load: impl FnOnce() -> Result<Option<V>, E>,
    ) -> Result<(Option<Arc<V>>, DeclarationCacheAccess), E> {
        // Find or create a stable per-key slot, then release the table lock before doing semantic
        // work. The write-side `entry` handles two unrelated readers racing to create the slot.
        let existing = {
            let entries = self
                .entries
                .read()
                .expect("declaration-cache map read lock should not be poisoned");
            entries.get(&key).cloned()
        };
        let slot = if let Some(slot) = existing {
            slot
        } else {
            self.entries
                .write()
                .expect("declaration-cache map write lock should not be poisoned")
                .entry(key)
                .or_insert_with(|| Arc::new(DeclarationSlot::default()))
                .clone()
        };

        // Initialized declarations, including a cached `None`, never take the per-key load lock.
        if let Some(value) = slot.value.get() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok((value.clone(), DeclarationCacheAccess::Hit));
        }

        // Exactly one contender loads this key. Recheck after locking because another contender
        // may have initialized the slot while this request was waiting.
        let _load = slot
            .load
            .lock()
            .expect("declaration-cache entry load lock should not be poisoned");
        if let Some(value) = slot.value.get() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok((value.clone(), DeclarationCacheAccess::Hit));
        }

        // Publish either the declaration or its stable absence. An error returns before `set`, so
        // a later request can retry the same key against a healthy package source.
        let value = load()?.map(Arc::new);
        assert!(
            slot.value.set(value.clone()).is_ok(),
            "declaration-cache entry should only initialize under its load lock"
        );
        self.misses.fetch_add(1, Ordering::Relaxed);
        Ok((value, DeclarationCacheAccess::Miss))
    }

    /// Fold low-contention hit and miss counters into the ordinary profile report.
    fn report(&self, hit_key: &'static str, miss_key: &'static str) {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        if hits != 0 {
            crate::profile::metric::DECLARATION_CACHE_ACCESSES.add(hit_key, hits);
        }
        if misses != 0 {
            crate::profile::metric::DECLARATION_CACHE_ACCESSES.add(miss_key, misses);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn concurrent_requests_lower_one_declaration_once() {
        let declarations = DeclarationMap::<u8, u8>::default();
        let load_count = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            let requests = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        declarations.get_or_try_init(7, || {
                            load_count.fetch_add(1, Ordering::Relaxed);
                            Ok::<_, Infallible>(Some(42))
                        })
                    })
                })
                .collect::<Vec<_>>();

            let mut accesses = requests
                .into_iter()
                .map(|request| {
                    let (value, access) = request
                        .join()
                        .expect("declaration-cache test worker should not panic")
                        .expect("infallible declaration load should succeed");
                    assert_eq!(value.as_deref(), Some(&42));
                    access
                })
                .collect::<Vec<_>>();
            accesses.sort_by_key(|access| matches!(access, DeclarationCacheAccess::Hit));

            assert_eq!(accesses[0], DeclarationCacheAccess::Miss);
            assert!(
                accesses[1..]
                    .iter()
                    .all(|access| *access == DeclarationCacheAccess::Hit)
            );
        });
        assert_eq!(load_count.load(Ordering::Relaxed), 1);
    }
}
