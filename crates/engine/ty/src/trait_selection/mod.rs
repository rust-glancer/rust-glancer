//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! This intentionally keeps a small project-facing facade around trait solving. The selector starts
//! from a resolved trait goal, uses the existing inference table to match direct impl-header
//! evidence, and then asks Chalk to prove the candidate's impl predicates in the default mode.
//! Callers that already have their own obligation/projection path can still opt into stricter
//! header-only selection or caller-owned predicate solving through `TraitSelectionOptions`.

use std::sync::{Arc, Mutex};

mod chalk;
mod header;
mod matcher;
mod predicate;
mod projection;

use rg_ir_model::{TraitApplicability, TraitImplRef, TraitRef};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery, TypePathContext,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use self::chalk::ChalkTraitSolver;
pub use self::header::TraitSelectionOptions;
use self::matcher::CandidateMatcher;
use self::predicate::{ImplPredicateProof, ImplPredicateProver};
pub use self::projection::AssocProjectionResult;
use crate::inference::{InferenceTable, InferenceTypeSubst};
use crate::{GenericArg, ItemPathQuery, Ty};

/// A shallow trait goal such as `Vec<?T>: FromIterator<User>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitGoal {
    pub self_ty: Ty,
    pub trait_ref: TraitRef,
    pub args: Vec<GenericArg>,
}

/// One `Trait<Assoc = Ty>` equality constraint carried by a trait goal.
pub(crate) struct AssocTypeConstraint<'a> {
    pub(crate) name: &'a Name,
    pub(crate) ty: Option<&'a Ty>,
}

impl TraitGoal {
    /// Iterate trait input args without associated-type equality constraints.
    ///
    /// Rust syntax puts both shapes inside the same angle brackets:
    ///
    /// ```text
    /// Iterator<Item = User>
    /// Indexed<Key, Item = User>
    /// ```
    ///
    /// Only the positional inputs belong in the trait substitution that Chalk sees as
    /// `Implemented(Self: Trait<...>)`. Associated equality args are separate projection
    /// constraints, such as `<Self as Iterator>::Item = User`.
    pub(crate) fn iter_positional_args(&self) -> impl Iterator<Item = &GenericArg> {
        self.args
            .iter()
            .filter(|arg| !matches!(arg, GenericArg::AssocType { .. }))
    }

    pub(crate) fn without_assoc_type_constraints(&self) -> Self {
        Self {
            self_ty: self.self_ty.clone(),
            trait_ref: self.trait_ref,
            args: self.iter_positional_args().cloned().collect(),
        }
    }

    pub(crate) fn has_assoc_type_constraints(&self) -> bool {
        self.args
            .iter()
            .any(|arg| matches!(arg, GenericArg::AssocType { .. }))
    }

    pub(crate) fn assoc_type_constraints(&self) -> impl Iterator<Item = AssocTypeConstraint<'_>> {
        self.args.iter().filter_map(|arg| {
            let GenericArg::AssocType { name, ty } = arg else {
                return None;
            };
            Some(AssocTypeConstraint {
                name,
                ty: ty.as_deref(),
            })
        })
    }
}

/// One visible impl whose header is compatible with a trait goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub subst: InferenceTypeSubst,
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
        subst: &InferenceTypeSubst,
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
            .as_mut()
            .expect("solver should be initialized before use");
        Ok(solver.impl_bounds_applicability(item_paths, trait_impl, impl_data, subst, table))
    }

    fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        context: TypePathContext,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error>
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
            .as_mut()
            .expect("solver should be initialized before use");
        Ok(solver.normalize_assoc_type(item_paths, context, goal, assoc_name, table))
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
    /// remains unchanged even if a candidate would solve variables.
    ///
    /// By default, multiple distinct concrete candidates become `ExpectedUnique::Ambiguous`, and
    /// speculative `Maybe` candidates are used only when no concrete candidate survives. Callers
    /// that need exploratory candidate sets can opt into keeping maybe candidates through
    /// `TraitSelectionOptions`.
    pub fn probe(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        let trait_impls = self.trait_impl_candidates(goal.trait_ref)?;
        let mut definite_selections = ExpectedUnique::new();
        let mut maybe_selections = ExpectedUnique::new();
        let mut all_selections = ExpectedUnique::new();
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
            if !self.options.prefers_definite_candidates() {
                all_selections.push(selection);
                continue;
            }
            if selection.applicability == TraitApplicability::Yes {
                definite_selections.push(selection);
            } else {
                maybe_selections.push(selection);
            }
        }

        // `Maybe` candidates come from unsupported or syntax-limited headers. They are useful for
        // exploratory callers, but commit-style trait selection should not let speculative headers
        // drown out a concrete match. Keep the ranking choice explicit in options so future callers
        // do not inherit a hidden policy by accident.
        if !self.options.prefers_definite_candidates() {
            return Ok(all_selections);
        }
        if !definite_selections.is_empty() {
            return Ok(definite_selections);
        }

        Ok(maybe_selections)
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
        let mut subst = InferenceTypeSubst::new();
        let matcher = CandidateMatcher::new(item_paths);
        let Some(applicability) =
            matcher.match_goal(goal, trait_impl, impl_data, &mut table, &mut subst)?
        else {
            return Ok(None);
        };
        let mut applicability = applicability;
        if !Self::apply_assoc_type_constraints(
            item_paths,
            target_items,
            goal,
            trait_impl,
            impl_data,
            &mut table,
            &mut applicability,
            cache,
        )? {
            return Ok(None);
        }

        if options.should_solve_impl_predicates() {
            if !Self::impl_has_chalk_predicates(impl_data) {
                crate::profile::metric::PREDICATE_FREE_CANDIDATES.inc();
                return Ok(applicability.is_applicable().then_some(TraitSelection {
                    trait_impl,
                    subst,
                    applicability,
                    table,
                }));
            }

            match ImplPredicateProver::new(item_paths)
                .prove_all_from_opaque_bounds(impl_data, &subst, &table)?
            {
                ImplPredicateProof::Proven(predicate_applicability) => {
                    applicability = applicability.and(predicate_applicability);
                }
                ImplPredicateProof::Rejected => return Ok(None),
                ImplPredicateProof::NotApplicable => {
                    let Some(predicate_applicability) = cache.impl_bounds_applicability(
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
                    applicability = applicability.and(predicate_applicability);
                }
            }
        }

        Ok(applicability.is_applicable().then_some(TraitSelection {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_assoc_type_constraints(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &rg_ir_model::hir::items::ImplData,
        table: &mut InferenceTable,
        applicability: &mut TraitApplicability,
        cache: &TraitSelectionCache,
    ) -> Result<bool, I::Error> {
        if !goal.has_assoc_type_constraints() {
            return Ok(true);
        }

        let projection_goal = goal.without_assoc_type_constraints();
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(trait_impl.impl_ref),
        };

        for constraint in goal.assoc_type_constraints() {
            let Some(expected_ty) = constraint.ty else {
                return Ok(false);
            };
            let Some(projection) = cache.normalize_assoc_type(
                item_paths,
                target_items,
                context,
                &projection_goal,
                constraint.name.as_str(),
                table,
            )?
            else {
                return Ok(false);
            };

            let mut projection_table = projection.table;
            if projection_table
                .try_unify(&projection.ty, expected_ty)
                .is_err()
            {
                return Ok(false);
            }
            *table = projection_table;
            *applicability = applicability.and(projection.applicability);
        }

        Ok(true)
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
