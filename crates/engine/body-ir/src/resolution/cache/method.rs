//! Negative extension-method results retained for one body.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use rg_ir_model::ScopeId;
use rg_text::Name;
use rg_ty::Ty;

/// Extension-trait misses retained for one body's inference lifetime.
///
/// The key is `(lexical scope, canonical receiver type, method name)`. For example, after proving
/// that scope 7 has no extension method `secret` for `Vec<u8>`, a later query can skip
/// the same trait search. If inference changes `Vec<?T>` into `Vec<u8>`, canonicalization produces a
/// different key and lookup runs again with the stronger evidence.
///
/// Only negative results live here. Positive candidates carry trial inference and selection state,
/// which is adapted directly into the call rather than retained by this declaration-level cache.
#[derive(Clone, Default)]
pub(crate) struct BodyMethodCache {
    shared: Arc<Mutex<BodyMethodCacheState>>,
}

/// Negative extension-method keys plus profiling counters for one body cache.
///
/// The nested maps keep scope and canonical receiver grouping explicit; the final `HashSet<Name>`
/// records method spellings that produced no callable trait candidate. Counters are emitted when
/// the shared state is dropped instead of touching global metrics on every lookup.
#[derive(Default)]
struct BodyMethodCacheState {
    trait_misses: HashMap<ScopeId, HashMap<Ty, HashSet<Name>>>,
    hits: usize,
    entries: usize,
}

impl BodyMethodCache {
    pub(crate) fn contains_trait_miss(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        method_name: &str,
    ) -> bool {
        let mut state = self
            .shared
            .lock()
            .expect("body method cache lock should not be poisoned");
        let found = state
            .trait_misses
            .get(&scope)
            .and_then(|by_receiver| by_receiver.get(receiver_ty))
            .is_some_and(|names| names.contains(method_name));
        if found {
            state.hits += 1;
        }
        found
    }

    pub(crate) fn remember_trait_miss(&self, scope: ScopeId, receiver_ty: Ty, method_name: &str) {
        let mut state = self
            .shared
            .lock()
            .expect("body method cache lock should not be poisoned");
        if state
            .trait_misses
            .entry(scope)
            .or_default()
            .entry(receiver_ty)
            .or_default()
            .insert(Name::new(method_name))
        {
            state.entries += 1;
        }
    }
}

impl Drop for BodyMethodCacheState {
    fn drop(&mut self) {
        if self.hits != 0 {
            crate::profile::metric::TRAIT_METHOD_MISS_CACHE_HITS.add(self.hits as u64);
        }
        if self.entries != 0 {
            crate::profile::metric::TRAIT_METHOD_MISS_CACHE_ENTRIES.add(self.entries as u64);
        }
    }
}
