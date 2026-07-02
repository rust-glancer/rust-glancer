//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! This intentionally keeps a small project-facing facade around trait solving. The selector starts
//! from a resolved trait goal, uses the existing inference table to match direct impl-header
//! evidence, and then asks Chalk to prove the candidate's where-clause obligations when the caller
//! wants full predicate solving. Callers that already have their own obligation/projection path can
//! still opt into header-only selection through `TraitSelectionOptions`.

use std::sync::{Arc, Mutex};

mod chalk;
mod header;
mod matcher;

use rg_ir_model::{TraitApplicability, TraitImplRef, TraitRef};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};
use rg_std::ExpectedUnique;

use self::chalk::ChalkTraitSolver;
pub use self::header::TraitSelectionOptions;
use self::matcher::CandidateMatcher;
use crate::ItemPathQuery;
use crate::inference::{InferGenericArg, InferTy, InferTypeSubst, InferenceTable};

/// A shallow trait goal such as `Vec<?T>: FromIterator<User>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitGoal {
    pub self_ty: InferTy,
    pub trait_ref: TraitRef,
    pub args: Vec<InferGenericArg>,
}

/// One visible impl whose header is compatible with a trait goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub subst: InferTypeSubst,
    pub applicability: TraitApplicability,
    /// Trial table after applying this candidate's direct equality evidence.
    ///
    /// Probe mode returns the table instead of mutating the caller. A later commit mode can adopt
    /// this table only when exactly one candidate survives.
    pub table: InferenceTable,
}

/// Reusable solver state for a group of trait-selection probes with the same visible items.
///
/// Building a Chalk program walks all visible stores, so the cost is much larger than checking one
/// candidate. Keep this cache scoped to the same target visibility context as the query that fills
/// it; different targets may see different impls and traits.
#[derive(Clone)]
pub struct TraitSelectionCache {
    solver: Arc<Mutex<Option<ChalkTraitSolver>>>,
}

impl Default for TraitSelectionCache {
    fn default() -> Self {
        Self {
            solver: Arc::new(Mutex::new(None)),
        }
    }
}

impl TraitSelectionCache {
    fn impl_bounds_applicability<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        trait_impl: TraitImplRef,
        impl_data: &rg_ir_model::hir::items::ImplData,
        subst: &InferTypeSubst,
        table: &InferenceTable,
    ) -> Result<Option<TraitApplicability>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut solver = self
            .solver
            .lock()
            .expect("trait selection solver cache lock should not be poisoned");
        if solver.is_none() {
            *solver = Some(ChalkTraitSolver::new(item_paths, target_items)?);
        }
        let solver = solver
            .as_ref()
            .expect("solver should be initialized before use");
        Ok(solver.impl_bounds_applicability(item_paths, trait_impl, impl_data, subst, table))
    }
}

/// Shared bounded trait-selection query.
pub struct TraitSelectionQuery<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
    options: TraitSelectionOptions,
    cache: TraitSelectionCache,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
            options: TraitSelectionOptions::new(),
            cache: TraitSelectionCache::default(),
        }
    }

    /// Use a non-default selection policy for all probes made through this query.
    pub fn with_options(mut self, options: TraitSelectionOptions) -> Self {
        self.options = options;
        self
    }

    /// Reuse solver state across probes made through this query.
    ///
    /// The cache belongs to the same visible item context as the query. Reusing it across bodies in
    /// one target is useful; reusing it across unrelated target visibility contexts would mix
    /// different impl universes.
    pub fn with_cache(mut self, cache: TraitSelectionCache) -> Self {
        self.cache = cache;
        self
    }

    /// Return the unique visible impl whose simple header is compatible with the goal.
    ///
    /// This is probe mode: every candidate gets a cloned inference table, and the caller's table
    /// remains unchanged even if a candidate would solve variables. Multiple distinct surviving
    /// candidates become `ExpectedUnique::Ambiguous` rather than being exposed as a ranking list.
    pub fn probe(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        let trait_impls = self.trait_impl_candidates(goal.trait_ref)?;
        let mut selections = ExpectedUnique::new();
        for trait_impl in trait_impls {
            let Some(selection) = Self::probe_impl(
                &self.item_paths,
                &self.target_items,
                goal,
                table,
                trait_impl,
                self.options,
                &self.cache,
            )?
            else {
                continue;
            };
            selections.push(selection);
        }
        Ok(selections)
    }

    /// Probe one already-visible impl against a trait goal.
    ///
    /// Method lookup and completion often start from an impl list that was already filtered by
    /// visibility, receiver indexes, or body-local overlay rules. This entry point lets those
    /// callers reuse the same bounded header matcher without asking trait selection to enumerate
    /// candidates again.
    pub fn probe_trait_impl(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<TraitSelection>, I::Error> {
        Self::probe_impl(
            &self.item_paths,
            &self.target_items,
            goal,
            table,
            trait_impl,
            self.options,
            &self.cache,
        )
    }

    /// Probe one already-visible impl using borrowed query state and an explicit policy.
    ///
    /// Some callers, such as method lookup, already own borrowed query state and want to reuse the
    /// same candidate matcher for a single impl. Keeping the options as a parameter is intentional:
    /// this helper is not a query-object method, so it must not smuggle in a different default
    /// policy than `probe` / `probe_trait_impl`.
    pub(crate) fn probe_visible_trait_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        options: TraitSelectionOptions,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let cache = TraitSelectionCache::default();
        Self::probe_impl(
            item_paths,
            target_items,
            goal,
            table,
            trait_impl,
            options,
            &cache,
        )
    }

    fn trait_impl_candidates(&self, trait_ref: TraitRef) -> Result<Vec<TraitImplRef>, I::Error> {
        Ok(self
            .lookup_index
            .trait_impls_for_trait(trait_ref)
            .map(|candidates| candidates.iter().copied().collect())
            .unwrap_or_default())
    }

    fn probe_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        options: TraitSelectionOptions,
        cache: &TraitSelectionCache,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let Some(impl_data) = target_items.items().impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&goal.trait_ref)
            || !options.accepts_impl_header(impl_data)
        {
            return Ok(None);
        }

        let mut table = table.clone();
        let mut subst = InferTypeSubst::new();
        let matcher = CandidateMatcher::new(item_paths);
        let Some(applicability) =
            matcher.match_goal(goal, trait_impl, impl_data, &mut table, &mut subst)?
        else {
            return Ok(None);
        };
        let mut applicability = applicability;

        if options.should_solve_where_predicates() {
            if !Self::impl_has_chalk_predicates(impl_data) {
                crate::profile::metric::PREDICATE_FREE_CANDIDATES.inc();
                return Ok(applicability.is_applicable().then_some(TraitSelection {
                    trait_impl,
                    subst,
                    applicability,
                    table,
                }));
            }

            let Some(where_applicability) = cache.impl_bounds_applicability(
                item_paths,
                target_items,
                trait_impl,
                impl_data,
                &subst,
                &table,
            )?
            else {
                return Ok(None);
            };
            applicability = applicability.and(where_applicability);
        }

        Ok(applicability.is_applicable().then_some(TraitSelection {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }

    fn impl_has_chalk_predicates(impl_data: &rg_ir_model::hir::items::ImplData) -> bool {
        impl_data
            .generics
            .types
            .iter()
            .any(|param| !param.bounds.is_empty())
            || !impl_data.generics.where_predicates.is_empty()
    }
}

#[cfg(test)]
mod tests;
