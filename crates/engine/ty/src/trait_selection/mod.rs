//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! This intentionally keeps a small project-facing facade around trait solving. The selector starts
//! from a resolved trait goal, uses the existing inference table to match direct impl-header
//! evidence, and then asks Chalk to prove the candidate's impl predicates in the default mode.
//! Callers that already have their own obligation/projection path can still opt into stricter
//! header-only selection or caller-owned predicate solving through `TraitSelectionOptions`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

mod chalk;
mod header;
mod matcher;
mod projection;

use rg_def_map::DefMapSource;
use rg_ir_model::{ImplRef, TraitApplicability, TraitDefRef, TraitImplRef, TypeAliasRef};
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};

use self::chalk::ChalkTraitSolver;
pub use self::header::TraitSelectionOptions;
use self::matcher::{CandidateMatcher, TraitImplCandidateIndex, TraitSelfHead};
pub use self::projection::AssocProjectionResult;
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::signature::impl_header_with;
use crate::{
    AssocTypeBinding, Clause, GenericArg, GenericArgs, ItemPathQuery, TraitApplication,
    TraitRefLowering, Ty, TypePathResolver,
};

/// A canonical trait application plus any associated-type equality constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub application: TraitApplication,
    pub associated_types: Vec<AssocTypeBinding>,
}

/// One `Trait<Assoc = Ty>` equality constraint carried by a trait goal.
pub(crate) struct AssocTypeConstraint<'a> {
    pub(crate) associated_ty: TypeAliasRef,
    pub(crate) ty: &'a Ty,
}

impl TraitGoal {
    /// Build a goal from positional arguments that do not include `Self`.
    pub fn new(
        self_ty: Ty,
        trait_ref: rg_ir_model::TraitDefRef,
        args: impl Into<GenericArgs>,
    ) -> Self {
        let args = args.into();
        let mut full_args = Vec::with_capacity(1 + args.len());
        full_args.push(GenericArg::Type(Box::new(self_ty)));
        full_args.extend(args.into_vec());
        Self {
            application: TraitApplication {
                def: trait_ref,
                args: full_args.into(),
            },
            associated_types: Vec::new(),
        }
    }

    pub fn from_lowering(lowering: TraitRefLowering) -> Self {
        Self {
            application: lowering.application,
            associated_types: lowering.associated_types,
        }
    }

    pub fn self_ty(&self) -> &Ty {
        self.application
            .self_ty()
            .expect("trait applications always contain the Self argument")
    }

    pub fn trait_ref(&self) -> rg_ir_model::TraitDefRef {
        self.application.def
    }

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
    pub fn iter_positional_args(&self) -> impl Iterator<Item = &GenericArg> {
        self.application.args.iter().skip(1)
    }

    pub(crate) fn without_assoc_type_constraints(&self) -> Self {
        Self {
            application: self.application.clone(),
            associated_types: Vec::new(),
        }
    }

    pub(crate) fn has_assoc_type_constraints(&self) -> bool {
        !self.associated_types.is_empty()
    }

    pub(crate) fn assoc_type_constraints(&self) -> impl Iterator<Item = AssocTypeConstraint<'_>> {
        self.associated_types
            .iter()
            .map(|binding| AssocTypeConstraint {
                associated_ty: binding.associated_ty,
                ty: &binding.ty,
            })
    }
}

/// One visible impl whose header is compatible with a trait goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub subst: InferenceSubstitution,
    pub applicability: TraitApplicability,
    /// Trial table after applying this candidate's direct equality evidence.
    ///
    /// Probe mode returns the table instead of mutating the caller. A later commit mode can adopt
    /// this table only when exactly one candidate survives.
    pub table: InferenceTable,
}

#[derive(Clone)]
struct CachedTraitSelection {
    trait_impl: TraitImplRef,
    subst: InferenceSubstitution,
    applicability: TraitApplicability,
}

impl CachedTraitSelection {
    fn from_selection(selection: TraitSelection) -> Self {
        Self {
            trait_impl: selection.trait_impl,
            subst: selection.subst,
            applicability: selection.applicability,
        }
    }

    fn with_table(self, table: InferenceTable) -> TraitSelection {
        TraitSelection {
            trait_impl: self.trait_impl,
            subst: self.subst,
            applicability: self.applicability,
            table,
        }
    }
}

/// Reusable solver state for a group of trait-selection probes with the same visible items.
///
/// Chalk program lowering follows every trait, impl, and opaque bound reachable from a goal, so its
/// cost is much larger than checking one candidate header. Keep this cache scoped to the same crate
/// visibility context as the query that fills it; different crates may see different impls.
#[derive(Clone)]
pub struct TraitSelectionCache {
    solver: Arc<ChalkTraitSolver>,
    impl_headers: Arc<Mutex<HashMap<ImplRef, Option<crate::ImplHeader>>>>,
    structural_trait_matches:
        Arc<Mutex<HashMap<(TraitImplRef, crate::AdtTy), Option<crate::Substitution>>>>,
    trait_impl_candidates:
        Arc<Mutex<HashMap<TraitDefRef, Arc<Mutex<Option<TraitImplCandidateIndex>>>>>>,
    strict_selections: Arc<Mutex<HashMap<TraitGoal, ExpectedUnique<CachedTraitSelection>>>>,
}

impl Default for TraitSelectionCache {
    fn default() -> Self {
        Self {
            solver: Arc::new(ChalkTraitSolver::new()),
            impl_headers: Arc::new(Mutex::new(HashMap::new())),
            structural_trait_matches: Arc::new(Mutex::new(HashMap::new())),
            trait_impl_candidates: Arc::new(Mutex::new(HashMap::new())),
            strict_selections: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TraitSelectionCache {
    fn impl_bounds_applicability<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Result<Option<TraitApplicability>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.solver
            .impl_bounds_applicability(item_paths, crate_items, self, clauses, subst, table)
    }

    fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.solver
            .normalize_assoc_type(item_paths, crate_items, self, goal, assoc_name, table)
    }

    /// Lower each canonical impl header once for this visible crate context.
    pub(crate) fn impl_header_with<'query, D, I, R>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        resolver: &R,
        impl_ref: ImplRef,
    ) -> Result<Option<crate::ImplHeader>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
        R: TypePathResolver<Error = I::Error>,
    {
        if let Some(header) = self
            .impl_headers
            .lock()
            .expect("trait selection impl-header cache lock should not be poisoned")
            .get(&impl_ref)
            .cloned()
        {
            return Ok(header);
        }

        let header = impl_header_with(item_paths, resolver, impl_ref)?;
        self.remember_impl_header(impl_ref, header.clone());
        Ok(header)
    }

    fn remember_impl_header(&self, impl_ref: ImplRef, header: Option<crate::ImplHeader>) {
        let mut headers = self
            .impl_headers
            .lock()
            .expect("trait selection impl-header cache lock should not be poisoned");
        // Parallel misses can finish out of order. A successfully lowered header may replace a
        // conservative miss, while a late miss must not erase a header another worker found.
        if header.is_some() {
            headers.insert(impl_ref, header);
        } else {
            headers.entry(impl_ref).or_insert(None);
        }
    }

    fn indexed_trait_impl_candidates<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
        visible_impls: &UniqueVec<TraitImplRef>,
        self_head: TraitSelfHead,
    ) -> Result<UniqueVec<TraitImplRef>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let index = self
            .trait_impl_candidates
            .lock()
            .expect("trait impl candidate-index map lock should not be poisoned")
            .entry(trait_ref)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();
        let mut index = index
            .lock()
            .expect("trait impl candidate-index lock should not be poisoned");
        if let Some(index) = index.as_ref() {
            return Ok(index.candidates(self_head));
        }

        // Lower every visible header for this trait once, then answer all later receiver queries
        // from its semantic `Self` fingerprint. The per-trait lock makes initialization
        // single-flight without serializing indexes for unrelated traits.
        let mut built = TraitImplCandidateIndex::default();
        for &trait_impl in visible_impls {
            let Some(header) =
                self.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
            else {
                continue;
            };
            built.push(trait_impl, &header);
        }

        let candidates = built.candidates(self_head);
        *index = Some(built);
        Ok(candidates)
    }

    fn prepare_trait_impl_predicates<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        trait_impls: &UniqueVec<TraitImplRef>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut clauses = Vec::new();
        for &trait_impl in trait_impls {
            let Some(header) =
                self.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
            else {
                continue;
            };
            clauses.extend(header.clauses);
        }
        self.solver
            .prepare_clauses(item_paths, crate_items, self, &clauses)
    }

    pub(crate) fn structural_trait_match(
        &self,
        trait_impl: TraitImplRef,
        receiver: &crate::AdtTy,
    ) -> Option<Option<crate::Substitution>> {
        self.structural_trait_matches
            .lock()
            .expect("structural trait-match cache lock should not be poisoned")
            .get(&(trait_impl, receiver.clone()))
            .cloned()
    }

    pub(crate) fn remember_structural_trait_match(
        &self,
        trait_impl: TraitImplRef,
        receiver: crate::AdtTy,
        subst: Option<crate::Substitution>,
    ) {
        self.structural_trait_matches
            .lock()
            .expect("structural trait-match cache lock should not be poisoned")
            .insert((trait_impl, receiver), subst);
    }

    fn strict_selection(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Option<ExpectedUnique<TraitSelection>> {
        self.strict_selections
            .lock()
            .expect("strict trait-selection cache lock should not be poisoned")
            .get(goal)
            .cloned()
            .map(|selection| selection.map(|selection| selection.with_table(table.clone())))
    }

    fn remember_strict_selection(
        &self,
        goal: TraitGoal,
        selection: &ExpectedUnique<TraitSelection>,
    ) {
        let selection = selection.clone().map(CachedTraitSelection::from_selection);
        self.strict_selections
            .lock()
            .expect("strict trait-selection cache lock should not be poisoned")
            .insert(goal, selection);
    }
}

/// Shared bounded trait-selection query.
pub struct TraitSelectionQuery<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    crate_items: CrateItemQuery<'query, D, I>,
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
        crate_items: CrateItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            crate_items,
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
    /// one crate is useful; reusing it across unrelated crate visibility contexts would mix
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
        // A goal that carries body-local inference or closure identity must be re-evaluated in its
        // owning body. Fully stable semantic goals cannot change the caller's table, so cache only
        // the selected impl/substitution and attach the caller's current table on a hit.
        let cacheable = self.options == TraitSelectionOptions::new()
            && goal
                .application
                .args
                .iter()
                .all(|arg| !arg.has_var() && !arg.has_closure())
            && goal
                .associated_types
                .iter()
                .all(|binding| !binding.ty.has_var() && !binding.ty.has_closure());
        if cacheable && let Some(selection) = self.cache.strict_selection(goal, table) {
            return Ok(selection);
        }

        let self_ty = table.resolve_root_var(goal.self_ty());
        let trait_impls = self.trait_impl_candidates(goal.trait_ref(), &self_ty)?;
        if self.options.should_solve_impl_predicates() {
            self.cache.prepare_trait_impl_predicates(
                &self.item_paths,
                &self.crate_items,
                &trait_impls,
            )?;
        }
        let mut definite_selections = ExpectedUnique::new();
        let mut maybe_selections = ExpectedUnique::new();
        let mut all_selections = ExpectedUnique::new();
        for trait_impl in trait_impls {
            let Some(selection) = Self::probe_impl(
                &self.item_paths,
                &self.crate_items,
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
        let selection = if !self.options.prefers_definite_candidates() {
            all_selections
        } else if !definite_selections.is_empty() {
            definite_selections
        } else {
            maybe_selections
        };
        if cacheable {
            self.cache
                .remember_strict_selection(goal.clone(), &selection);
        }
        Ok(selection)
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
            &self.crate_items,
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
        crate_items: &CrateItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        header: &crate::ImplHeader,
        options: TraitSelectionOptions,
        cache: &TraitSelectionCache,
    ) -> Result<Option<TraitSelection>, I::Error> {
        cache.remember_impl_header(trait_impl.impl_ref, Some(header.clone()));
        Self::probe_impl_with_header(
            item_paths,
            crate_items,
            goal,
            table,
            trait_impl,
            header,
            options,
            cache,
        )
    }

    fn trait_impl_candidates(
        &self,
        trait_ref: TraitDefRef,
        self_ty: &Ty,
    ) -> Result<UniqueVec<TraitImplRef>, I::Error> {
        let Some(visible_impls) = self.lookup_index.trait_impls_for_trait(trait_ref) else {
            return Ok(UniqueVec::new());
        };
        let Some(self_head) = TraitSelfHead::from_ty(self_ty) else {
            return Ok(visible_impls.clone());
        };
        self.cache.indexed_trait_impl_candidates(
            &self.item_paths,
            trait_ref,
            visible_impls,
            self_head,
        )
    }

    fn probe_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        options: TraitSelectionOptions,
        cache: &TraitSelectionCache,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let Some(header) = cache.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
        else {
            return Ok(None);
        };
        Self::probe_impl_with_header(
            item_paths,
            crate_items,
            goal,
            table,
            trait_impl,
            &header,
            options,
            cache,
        )
    }

    /// Probe using the canonical header already lowered by the caller's resolution layer.
    #[allow(clippy::too_many_arguments)]
    fn probe_impl_with_header(
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        header: &crate::ImplHeader,
        options: TraitSelectionOptions,
        cache: &TraitSelectionCache,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let Some(impl_data) = crate_items.items().impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&goal.trait_ref())
            || !options.accepts_impl_header(impl_data)
        {
            return Ok(None);
        }

        let mut table = table.clone();
        let mut subst = InferenceSubstitution::new();
        let matcher = CandidateMatcher;
        let Some(applicability) =
            matcher.match_goal(goal, trait_impl, header, &mut table, &mut subst)
        else {
            return Ok(None);
        };
        let mut applicability = applicability;
        if !Self::apply_assoc_type_constraints(
            item_paths,
            crate_items,
            goal,
            trait_impl,
            &subst,
            &mut table,
            &mut applicability,
            cache,
        )? {
            return Ok(None);
        }

        if options.should_solve_impl_predicates() {
            if header.clauses.is_empty() {
                crate::profile::metric::PREDICATE_FREE_CANDIDATES.inc();
                return Ok(applicability.is_applicable().then_some(TraitSelection {
                    trait_impl,
                    subst,
                    applicability,
                    table,
                }));
            }

            let Some(predicate_applicability) = cache.impl_bounds_applicability(
                item_paths,
                crate_items,
                &header.clauses,
                &subst,
                &table,
            )?
            else {
                return Ok(None);
            };
            applicability = applicability.and(predicate_applicability);
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
        crate_items: &CrateItemQuery<'query, D, I>,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        subst: &InferenceSubstitution,
        table: &mut InferenceTable,
        applicability: &mut TraitApplicability,
        cache: &TraitSelectionCache,
    ) -> Result<bool, I::Error> {
        if !goal.has_assoc_type_constraints() {
            return Ok(true);
        }

        let projection_goal = goal.without_assoc_type_constraints();
        for constraint in goal.assoc_type_constraints() {
            let Some(alias_data) = crate_items
                .items()
                .type_alias_data(constraint.associated_ty)?
            else {
                return Ok(false);
            };

            // The candidate has already supplied its impl substitution. Most associated values,
            // such as `impl<T> Iterator for Iter<T> { type Item = T; }`, can therefore be checked
            // directly without asking the global solver to select the same impl again. Nested
            // projections and trait defaults still fall through to Chalk below.
            if let Some(ty) = Self::canonical_impl_assoc_value(
                item_paths,
                trait_impl.impl_ref,
                subst,
                alias_data.name.as_str(),
            )? && !ty.has_projection()
            {
                if table.try_unify(&ty, constraint.ty).is_err() {
                    return Ok(false);
                }
                continue;
            }

            let Some(projection) = cache.normalize_assoc_type(
                item_paths,
                crate_items,
                &projection_goal,
                alias_data.name.as_str(),
                table,
            )?
            else {
                return Ok(false);
            };

            let mut projection_table = projection.table;
            if projection_table
                .try_unify(&projection.ty, constraint.ty)
                .is_err()
            {
                return Ok(false);
            }
            *table = projection_table;
            *applicability = applicability.and(projection.applicability);
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests;
