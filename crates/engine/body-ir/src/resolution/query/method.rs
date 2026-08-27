//! Method lookup for receiver types.
//!
//! Ref-level member lookup can stop after identifying a function. Body inference also keeps the
//! receiver substitutions and trait-selection evidence needed to instantiate its signature. For
//! `[User; 3].into_iter()`, the selected array impl carries `T = User` and `N = 3`, allowing the
//! return projection to become `array::IntoIter<User, 3>`.
//!
//! A named call walks autoderef depths in order. At each depth it tries inherent methods first and
//! opens the lexically visible trait lane only if no inherent method with that name applies. The
//! first depth with candidates wins. Broad completion uses a separate path that can return methods
//! from every reachable depth and both declaration families.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use rg_def_map::DefMapSource;
use rg_ir_model::ScopeId;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_text::Name;
use rg_ty::{
    AutoderefMode, MemberMethodCandidateRef, MemberMethodOrigin, Ty, inference::InferenceTable,
};

use crate::resolution::BodyResolutionContext;

use super::{BodyCallableCandidate, BodyReceiverImplMatches};

/// Extension-trait misses retained for one body's inference lifetime.
///
/// The key is `(lexical scope, canonical receiver type, method name)`. For example, after proving
/// that scope 7 has no extension method `secret` for `Vec<u8>`, a later fixed-point round can skip
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
    fn contains_trait_miss(&self, scope: ScopeId, receiver_ty: &Ty, method_name: &str) -> bool {
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

    fn remember_trait_miss(&self, scope: ScopeId, receiver_ty: Ty, method_name: &str) {
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

/// Resolves method declarations while preserving the evidence needed by body inference.
///
/// `method_candidates_for_ty` serves broad editor completion. `named_method_candidates_for_ty`
/// implements call lookup order and returns body callables with receiver substitutions and trait
/// selections attached.
pub struct BodyMethodQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyMethodQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return all methods that can be reached from this receiver type.
    pub fn method_candidates_for_ty(
        &self,
        scope: ScopeId,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let table = InferenceTable::new();
        let mut candidates = Vec::new();
        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            let receiver = self.context.impls().matches_for_receiver_with_functions(
                scope,
                candidate.ty(),
                &table,
            )?;
            for function in matcher.function_candidates_for_matches(receiver.matches(), None)? {
                let Some(function_data) = self
                    .context
                    .item_query()
                    .function_data(function.function())?
                else {
                    continue;
                };
                if !function_data.has_self_receiver()
                    || receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
                {
                    continue;
                }

                let candidate = match function.trait_selection() {
                    Some(selection) => MemberMethodCandidateRef::trait_method(
                        function.function(),
                        selection.applicability,
                    ),
                    None => MemberMethodCandidateRef::inherent(function.function()),
                };
                Self::push_candidate(&mut candidates, candidate);
            }
        }

        Ok(candidates)
    }

    /// Return named method candidates at the first matching autoderef depth.
    ///
    /// For `&&Widget::render`, lookup tries `&&Widget`, then `&Widget`, then `Widget`, stopping at
    /// the first depth that exposes `render`. At one depth an inherent `render` wins without proving
    /// same-name extension traits; if no inherent declaration applies, only lexically visible
    /// traits are considered.
    pub(crate) fn named_method_candidates_for_ty(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        method_name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        let mut current_depth = None;
        let mut candidates = UniqueVec::new();

        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, receiver_ty)
        {
            let candidate = candidate?;
            // Method calls select the first autoderef depth that has matching methods. Completion
            // can be more generous, but call inference must not mix receiver substitutions across
            // different depths.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && !candidates.is_empty()
            {
                return Ok(candidates);
            }
            current_depth = Some(candidate.depth());

            // Rust probes inherent methods before extension traits for one receiver adjustment.
            // Most calls settle here, so avoid proving every same-name trait impl merely to
            // discard it behind an inherent declaration.
            let inherent_receiver = self
                .context
                .impls()
                .inherent_matches_for_receiver(candidate.ty())?;
            if self.extend_named_callable_candidates(
                &mut candidates,
                &inherent_receiver,
                method_name,
            )? {
                continue;
            }

            // The inference table participates only in the cache key. Matching must keep the
            // original receiver so a method such as `Vec<?T>::push(T)` can still constrain `?T`
            // from later arguments. When that slot is solved, canonicalization produces a new key
            // and the trait lane is retried with the stronger receiver.
            let cache_receiver_ty = table.canonicalize(candidate.ty());
            if self.context.method_cache().contains_trait_miss(
                scope,
                &cache_receiver_ty,
                method_name,
            ) {
                continue;
            }

            let trait_receiver = self
                .context
                .impls()
                .trait_matches_for_receiver_with_function_name(
                    scope,
                    candidate.ty(),
                    method_name,
                    table,
                )?;
            if !self.extend_named_callable_candidates(
                &mut candidates,
                &trait_receiver,
                method_name,
            )? {
                self.context.method_cache().remember_trait_miss(
                    scope,
                    cache_receiver_ty,
                    method_name,
                );
            }
        }

        Ok(candidates)
    }

    /// Adapt one already-selected impl lane into callable method candidates.
    ///
    /// The boolean reports that this lane exposed a method even when another receiver adjustment
    /// already inserted the same callable candidate.
    fn extend_named_callable_candidates(
        &self,
        candidates: &mut UniqueVec<BodyCallableCandidate>,
        receiver: &BodyReceiverImplMatches,
        method_name: &str,
    ) -> Result<bool, PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        let mut found = false;
        for function in
            matcher.function_candidates_for_matches(receiver.matches(), Some(method_name))?
        {
            let Some(function_data) = item_query.function_data(function.function())? else {
                continue;
            };
            if function_data.name != method_name
                || !function_data.has_self_receiver()
                || receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
            {
                continue;
            }

            let Some(candidate) = BodyCallableCandidate::from_receiver_function(
                &self.context,
                receiver.receiver_ty(),
                function,
                None,
            )?
            else {
                continue;
            };
            found = true;
            candidates.push(candidate);
        }
        Ok(found)
    }

    /// Deduplicate a method candidate and keep the stronger origin.
    fn push_candidate(
        candidates: &mut Vec<MemberMethodCandidateRef>,
        candidate: MemberMethodCandidateRef,
    ) {
        let Some(existing) = candidates
            .iter_mut()
            .find(|existing| existing.function() == candidate.function())
        else {
            candidates.push(candidate);
            return;
        };

        *existing = Self::merge_candidates(*existing, candidate);
    }

    /// Merge duplicate candidates from inherent and trait lookup.
    fn merge_candidates(
        left: MemberMethodCandidateRef,
        right: MemberMethodCandidateRef,
    ) -> MemberMethodCandidateRef {
        match (left.origin(), right.origin()) {
            (MemberMethodOrigin::Inherent, _) => left,
            (_, MemberMethodOrigin::Inherent) => right,
            (
                MemberMethodOrigin::Trait {
                    applicability: left_applicability,
                },
                MemberMethodOrigin::Trait {
                    applicability: right_applicability,
                },
            ) => MemberMethodCandidateRef::trait_method(
                left.function(),
                left_applicability.or(right_applicability),
            ),
        }
    }
}
