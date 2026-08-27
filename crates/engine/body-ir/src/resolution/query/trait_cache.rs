//! Body-owned caches for lexical trait scope and declaration surfaces.
//!
//! A method lookup first finds traits that declare the requested item, then intersects them with
//! the traits visible at the call site. Both sets are stable for one immutable body even while
//! inference revisits the lookup, so this cache owns them together for exactly that body lifetime.
//!
//! For `value.render()`, the cached work is:
//!
//! 1. collect traits visible at the expression's lexical scope;
//! 2. find traits whose declaration surface contains a function named `render`;
//! 3. retain the intersection of those sets.
//!
//! Receiver matching is deliberately not cached here. `value` may change from `Vec<?T>` to
//! `Vec<u8>` while inference runs, whereas declaration names and lexical imports cannot change.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use rg_ir_model::{ScopeId, TraitDefRef};
use rg_std::UniqueVec;
use rg_text::Name;

/// Successful lexical trait sets and filtered declaration surfaces retained for one body.
///
/// Body resolution recreates query contexts while its fixed point refines types. Those contexts
/// share this cache, so `value.render()` does not repeat either DefMap scope collection or the
/// declaration-surface intersection on every round. A new body or request receives a new handle.
#[derive(Clone, Default)]
pub(crate) struct BodyTraitLookupCache {
    shared: Arc<BodyTraitLookupCacheShared>,
}

#[derive(Default)]
struct BodyTraitLookupCacheShared {
    /// Low-frequency scope and declaration results, plus their batched profiling counters.
    state: Mutex<BodyTraitLookupCacheState>,
    /// A hot-path counter kept outside the mutex and flushed when the body cache is dropped.
    empty_extension_probes: AtomicUsize,
}

/// Published lexical sets and declaration surfaces for one immutable body.
///
/// For a scope containing `use api::{Inspect, Render};`, `scopes` stores both trait identities. A
/// later `value.render()` probe may add `FunctionNamed("render") -> [Render]` under the same scope
/// in `surfaces`. The hit/miss counters stay beside the cached data and are emitted once on drop so
/// normal lookup does not update global metrics on every access.
#[derive(Default)]
struct BodyTraitLookupCacheState {
    scopes: HashMap<ScopeId, Arc<HashSet<TraitDefRef>>>,
    surfaces: HashMap<ScopeId, CachedTraitSurfaces>,
    scope_hits: usize,
    scope_misses: usize,
    surface_hits: usize,
    surface_misses: usize,
}

/// One lexical scope's declaration surfaces after intersecting them with its visible traits.
///
/// Each slot represents a different way a caller enters trait lookup. Broad completion fills a
/// broad slot once, while `value.render()` and `Type::MAX` use their own named entries. These lists
/// contain trait declarations only; receiver-specific impl proof happens after the cache boundary.
#[derive(Default)]
struct CachedTraitSurfaces {
    associated_items: Option<Arc<UniqueVec<TraitDefRef>>>,
    functions: Option<Arc<UniqueVec<TraitDefRef>>>,
    functions_by_name: HashMap<Name, Arc<UniqueVec<TraitDefRef>>>,
    consts_by_name: HashMap<Name, Arc<UniqueVec<TraitDefRef>>>,
}

#[derive(Clone, Copy)]
pub(super) enum BodyTraitSurface<'name> {
    /// Traits declaring any associated item, used by broad associated-item lookup.
    AssociatedItems,
    /// Traits declaring at least one function, used by method completion.
    Functions,
    /// Traits declaring a particular method name, such as `render` in `value.render()`.
    FunctionNamed(&'name str),
    /// Traits declaring a particular const name, such as `MAX` in associated-item lookup.
    ConstNamed(&'name str),
}

impl CachedTraitSurfaces {
    fn get(&self, surface: BodyTraitSurface<'_>) -> Option<Arc<UniqueVec<TraitDefRef>>> {
        match surface {
            BodyTraitSurface::AssociatedItems => self.associated_items.clone(),
            BodyTraitSurface::Functions => self.functions.clone(),
            BodyTraitSurface::FunctionNamed(name) => self.functions_by_name.get(name).cloned(),
            BodyTraitSurface::ConstNamed(name) => self.consts_by_name.get(name).cloned(),
        }
    }

    fn insert(&mut self, surface: BodyTraitSurface<'_>, traits: Arc<UniqueVec<TraitDefRef>>) {
        match surface {
            BodyTraitSurface::AssociatedItems => self.associated_items = Some(traits),
            BodyTraitSurface::Functions => self.functions = Some(traits),
            BodyTraitSurface::FunctionNamed(name) => {
                self.functions_by_name.insert(Name::new(name), traits);
            }
            BodyTraitSurface::ConstNamed(name) => {
                self.consts_by_name.insert(Name::new(name), traits);
            }
        }
    }
}

impl BodyTraitLookupCache {
    /// Return one cached scope, publishing only a complete successful collection.
    pub(super) fn scope_or_try_init<E>(
        &self,
        scope: ScopeId,
        collect: impl FnOnce() -> Result<HashSet<TraitDefRef>, E>,
    ) -> Result<Arc<HashSet<TraitDefRef>>, E> {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("body trait-lookup cache lock should not be poisoned");
            if let Some(traits) = state.scopes.get(&scope).cloned() {
                state.scope_hits += 1;
                return Ok(traits);
            }
        }

        let collected = Arc::new(collect()?);
        let mut state = self
            .shared
            .state
            .lock()
            .expect("body trait-lookup cache lock should not be poisoned");
        if let Some(traits) = state.scopes.get(&scope).cloned() {
            // Body resolution is single-threaded, but keep clones race-safe for finalized query
            // consumers without publishing two different values for one scope.
            state.scope_hits += 1;
            return Ok(traits);
        }
        state.scope_misses += 1;
        state.scopes.insert(scope, Arc::clone(&collected));
        Ok(collected)
    }

    /// Reuse the trait candidates produced by one declaration surface and lexical scope.
    ///
    /// `value.run()` can be revisited by many fixed-point rounds. Its declaring-trait union and
    /// lexical intersection are stable for the immutable body, even while receiver inference is
    /// still changing. Named maps accept borrowed strings on hits and allocate a `Name` only once.
    pub(super) fn surface_or_try_init<E>(
        &self,
        scope: ScopeId,
        surface: BodyTraitSurface<'_>,
        collect: impl FnOnce() -> Result<UniqueVec<TraitDefRef>, E>,
    ) -> Result<Arc<UniqueVec<TraitDefRef>>, E> {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("body trait-lookup cache lock should not be poisoned");
            if let Some(traits) = state
                .surfaces
                .get(&scope)
                .and_then(|surfaces| surfaces.get(surface))
            {
                state.surface_hits += 1;
                return Ok(traits);
            }
        }

        let collected = Arc::new(collect()?);
        let mut state = self
            .shared
            .state
            .lock()
            .expect("body trait-lookup cache lock should not be poisoned");
        if let Some(traits) = state
            .surfaces
            .get(&scope)
            .and_then(|surfaces| surfaces.get(surface))
        {
            state.surface_hits += 1;
            return Ok(traits);
        }
        state.surface_misses += 1;
        state
            .surfaces
            .entry(scope)
            .or_default()
            .insert(surface, Arc::clone(&collected));
        Ok(collected)
    }

    /// Record a named method surface that contained no lexically visible trait.
    pub(super) fn record_empty_extension_probe(&self) {
        self.shared
            .empty_extension_probes
            .fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn stats(&self) -> (usize, usize, usize, usize) {
        let state = self
            .shared
            .state
            .lock()
            .expect("body trait-lookup cache lock should not be poisoned");
        (
            state.scope_hits,
            state.scope_misses,
            state.surface_hits,
            state.surface_misses,
        )
    }
}

impl Drop for BodyTraitLookupCacheShared {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .expect("body trait-lookup cache lock should not be poisoned");
        if state.scope_hits != 0 {
            crate::profile::metric::TRAIT_SCOPE_CACHE_HITS.add(state.scope_hits as u64);
        }
        if state.scope_misses != 0 {
            crate::profile::metric::TRAIT_SCOPE_CACHE_MISSES.add(state.scope_misses as u64);
        }
        if state.surface_hits != 0 {
            crate::profile::metric::TRAIT_SURFACE_CACHE_HITS.add(state.surface_hits as u64);
        }
        if state.surface_misses != 0 {
            crate::profile::metric::TRAIT_SURFACE_CACHE_MISSES.add(state.surface_misses as u64);
        }
        let empty_extension_probes = self.empty_extension_probes.load(Ordering::Relaxed);
        if empty_extension_probes != 0 {
            crate::profile::metric::EMPTY_TRAIT_EXTENSION_SURFACES
                .add(empty_extension_probes as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn trait_cache_reuses_one_scope_but_not_another() {
        let cache = BodyTraitLookupCache::default();
        let loads = Cell::new(0);

        let first = cache
            .scope_or_try_init(ScopeId(3), || {
                loads.set(loads.get() + 1);
                Ok::<_, &'static str>(HashSet::new())
            })
            .expect("first scope collection should succeed");
        let repeated = cache
            .scope_or_try_init(ScopeId(3), || {
                loads.set(loads.get() + 1);
                Ok::<_, &'static str>(HashSet::new())
            })
            .expect("repeated scope collection should succeed");
        let other = cache
            .scope_or_try_init(ScopeId(7), || {
                loads.set(loads.get() + 1);
                Ok::<_, &'static str>(HashSet::new())
            })
            .expect("other scope collection should succeed");

        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(loads.get(), 2);
        assert_eq!(cache.stats(), (1, 2, 0, 0));
    }

    #[test]
    fn trait_cache_does_not_remember_errors() {
        let cache = BodyTraitLookupCache::default();

        let failed = cache.scope_or_try_init(ScopeId(4), || {
            Err::<HashSet<TraitDefRef>, _>("scope unavailable")
        });
        assert_eq!(
            failed.expect_err("first collection should fail"),
            "scope unavailable"
        );

        cache
            .scope_or_try_init(ScopeId(4), || Ok::<_, &'static str>(HashSet::new()))
            .expect("a later successful collection should retry");
        assert_eq!(cache.stats(), (0, 1, 0, 0));
    }

    #[test]
    fn trait_cache_reuses_filtered_named_surfaces() {
        let cache = BodyTraitLookupCache::default();
        let loads = Cell::new(0);

        let first = cache
            .surface_or_try_init(
                ScopeId(5),
                BodyTraitSurface::FunctionNamed("render"),
                || {
                    loads.set(loads.get() + 1);
                    Ok::<_, &'static str>(UniqueVec::new())
                },
            )
            .expect("first surface collection should succeed");
        let repeated = cache
            .surface_or_try_init(
                ScopeId(5),
                BodyTraitSurface::FunctionNamed("render"),
                || {
                    loads.set(loads.get() + 1);
                    Ok::<_, &'static str>(UniqueVec::new())
                },
            )
            .expect("repeated surface collection should succeed");
        cache
            .surface_or_try_init(ScopeId(5), BodyTraitSurface::Functions, || {
                loads.set(loads.get() + 1);
                Ok::<_, &'static str>(UniqueVec::new())
            })
            .expect("a distinct surface should collect independently");

        assert!(Arc::ptr_eq(&first, &repeated));
        assert_eq!(loads.get(), 2);
        assert_eq!(cache.stats(), (0, 0, 1, 2));
    }
}
