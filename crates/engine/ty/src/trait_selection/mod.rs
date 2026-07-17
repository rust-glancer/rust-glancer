//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! Native matching discovers canonical impl headers that may fit a resolved trait goal. Chalk then
//! owns proof of their predicates and associated-type equalities. Keeping those phases as different
//! types prevents exploratory editor candidates from being mistaken for established semantic facts.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

mod candidate;
mod chalk;
mod matcher;
mod projection;

use rg_def_map::DefMapSource;
use rg_ir_model::{CrateRef, ImplRef, TraitApplicability, TraitDefRef, TraitImplRef, TypeAliasRef};
use rg_semantic_ir::{CrateItemQuery, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};

pub use self::candidate::{TraitCandidate, TraitCandidateQuery};
use self::chalk::{ChalkOutcome, ChalkTraitSolver};
use self::matcher::{TraitImplCandidateIndex, TraitSelfHead};
pub use self::projection::AssocProjectionResult;
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::signature::impl_header_with;
use crate::{
    AssocTypeBinding, Clause, GenericArg, GenericArgs, ItemPathQuery, Substitution,
    TraitApplication, TraitRefLowering, Ty, TyContext, TypePathResolver,
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

    /// Return whether this goal is independent of one body's live inference state.
    ///
    /// Semantic unknowns and projections are stable values: a later, more precise query produces a
    /// different goal. Inference variables and closure identities instead belong to the caller's
    /// table/body and must be classified again there.
    fn is_cache_stable(&self) -> bool {
        self.application
            .args
            .iter()
            .all(|arg| !arg.has_var() && !arg.has_closure())
            && self
                .associated_types
                .iter()
                .all(|binding| !binding.ty.has_var() && !binding.ty.has_closure())
    }
}

/// One trait impl whose predicates were submitted to the shared solver.
///
/// Unlike [`TraitCandidate`], this value is safe for semantic consumers. `Maybe` means Chalk or
/// canonical header matching found genuine ambiguity; it does not mean that predicate proof was
/// silently delegated to another caller.
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

/// Internal result that keeps an implementation limit separate from a semantic rejection.
enum SemanticOutcome<T> {
    Available(T),
    Rejected,
    Unavailable,
}

type SharedTraitImplCandidateIndex = Arc<Mutex<Option<TraitImplCandidateIndex>>>;
type TraitImplCandidateIndexes = Arc<Mutex<HashMap<TraitDefRef, SharedTraitImplCandidateIndex>>>;
type ExactCandidateApplicabilities =
    Arc<Mutex<HashMap<TraitImplRef, HashMap<TraitGoal, TraitApplicability>>>>;

/// Reusable solver session for trait-selection probes with the same visible items.
///
/// Chalk program lowering follows every trait, impl, and opaque bound reachable from a goal, so its
/// cost is much larger than checking one candidate header. The use-site crate is part of the
/// session identity because different crates may see different impl universes for the same goal.
/// Cloning a session only clones shared handles: the Chalk program, candidate indexes, and exact
/// answers remain shared by every `TyContext` for that use site.
#[derive(Clone)]
pub struct TraitSelectionSession {
    use_site: CrateRef,
    solver: Arc<ChalkTraitSolver>,
    impl_headers: Arc<Mutex<HashMap<ImplRef, Option<crate::ImplHeader>>>>,
    trait_impl_candidates: TraitImplCandidateIndexes,
    strict_selections: Arc<Mutex<HashMap<TraitGoal, ExpectedUnique<CachedTraitSelection>>>>,
    exact_candidate_applicabilities: ExactCandidateApplicabilities,
}

impl fmt::Debug for TraitSelectionSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraitSelectionSession")
            .field("use_site", &self.use_site)
            .finish_non_exhaustive()
    }
}

impl TraitSelectionSession {
    pub fn new(use_site: CrateRef) -> Self {
        Self {
            use_site,
            solver: Arc::new(ChalkTraitSolver::new()),
            impl_headers: Arc::new(Mutex::new(HashMap::new())),
            trait_impl_candidates: Arc::new(Mutex::new(HashMap::new())),
            strict_selections: Arc::new(Mutex::new(HashMap::new())),
            exact_candidate_applicabilities: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn use_site(&self) -> CrateRef {
        self.use_site
    }

    fn prove_impl_bounds<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<()>, I::Error>
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
        selected_impl: Option<(ImplRef, &InferenceSubstitution)>,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<AssocProjectionResult>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.solver.normalize_assoc_type(
            item_paths,
            crate_items,
            self,
            goal,
            assoc_name,
            selected_impl,
            table,
        )
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

    fn exact_candidate_applicability(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
    ) -> Option<TraitApplicability> {
        if !goal.is_cache_stable() {
            return None;
        }
        self.exact_candidate_applicabilities
            .lock()
            .expect("exact trait-candidate cache lock should not be poisoned")
            .get(&trait_impl)?
            .get(goal)
            .copied()
    }

    fn remember_exact_candidate_applicability(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        applicability: TraitApplicability,
    ) {
        if !goal.is_cache_stable() {
            return;
        }
        self.exact_candidate_applicabilities
            .lock()
            .expect("exact trait-candidate cache lock should not be poisoned")
            .entry(trait_impl)
            .or_default()
            .insert(goal.clone(), applicability);
    }
}

/// Shared bounded trait-selection query.
pub struct TraitSelectionQuery<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return the unique visible impl whose header fits and whose predicates Chalk can prove.
    ///
    /// This is probe mode: every candidate gets a cloned inference table, and the caller's table
    /// remains unchanged even if a candidate would solve variables.
    ///
    /// Multiple distinct concrete selections become `ExpectedUnique::Ambiguous`. Speculative
    /// `Maybe` selections are used only when no concrete selection survives. Editor-facing callers
    /// that want every header match should use [`TraitCandidateQuery`] instead.
    pub fn probe(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        // A goal that carries body-local inference or closure identity must be re-evaluated in its
        // owning body. Fully stable semantic goals cannot change the caller's table, so cache only
        // the selected impl/substitution and attach the caller's current table on a hit.
        let cacheable = goal.is_cache_stable();
        if cacheable
            && let Some(selection) = self.context.trait_selection().strict_selection(goal, table)
        {
            return Ok(selection);
        }

        let candidates = TraitCandidateQuery::probe_all_with(
            self.context.item_paths(),
            self.context.lookup_index(),
            self.context.trait_selection(),
            goal,
            table,
        )?;

        // Program lowering follows clauses transitively. Prime all candidate predicates once so
        // individual proof attempts do not repeatedly extend the program under the solver lock.
        let mut trait_impls = UniqueVec::new();
        for candidate in &candidates {
            trait_impls.push(candidate.trait_impl);
        }
        self.context
            .trait_selection()
            .prepare_trait_impl_predicates(
                self.context.item_paths(),
                self.context.crate_items(),
                &trait_impls,
            )?;

        let mut definite_selections = ExpectedUnique::new();
        let mut maybe_selections = ExpectedUnique::new();
        let mut fully_evaluated = true;
        for candidate in candidates {
            let selection = match self.select_candidate(goal, candidate)? {
                SemanticOutcome::Available(selection) => selection,
                SemanticOutcome::Rejected => continue,
                SemanticOutcome::Unavailable => {
                    fully_evaluated = false;
                    continue;
                }
            };
            if selection.applicability == TraitApplicability::Yes {
                definite_selections.push(selection);
            } else {
                maybe_selections.push(selection);
            }
        }

        // A speculative header or ambiguous proof must not drown out a concrete result. This
        // ranking belongs to semantic selection; exploratory discovery exposes all candidates.
        let selection = if !definite_selections.is_empty() {
            definite_selections
        } else {
            maybe_selections
        };
        if cacheable && fully_evaluated {
            self.context
                .trait_selection()
                .remember_strict_selection(goal.clone(), &selection);
        }
        Ok(selection)
    }

    /// Classify an impl that receiver matching has already instantiated.
    ///
    /// Method lookup starts from one indexed impl and matches its `Self` header against the
    /// receiver. That match already supplies every substitution needed to instantiate the impl's
    /// own trait application, so rediscovering the same impl through native goal matching would be
    /// duplicate work. This entry point starts at the semantic boundary that remains: proving the
    /// instantiated predicates and associated-type constraints.
    ///
    /// Stable exact classifications are shared across fixed-point passes. Definite rejection and
    /// genuine proof ambiguity are cacheable; adapter limits remain an uncached `Maybe` so a later
    /// query can retry instead of treating bounded work exhaustion as a semantic fact.
    pub(crate) fn instantiated_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        header: &crate::ImplHeader,
        subst: Substitution,
    ) -> Result<TraitApplicability, I::Error> {
        let Some(mut trait_ref) = header.trait_ref.clone() else {
            return Ok(TraitApplicability::No);
        };
        trait_ref.application.args = trait_ref
            .application
            .args
            .iter()
            .map(|arg| subst.apply_arg(arg))
            .collect();
        trait_ref.associated_types = trait_ref
            .associated_types
            .into_iter()
            .map(|binding| AssocTypeBinding {
                associated_ty: binding.associated_ty,
                ty: subst.apply(&binding.ty),
            })
            .collect();
        let goal = TraitGoal::from_lowering(trait_ref);

        if let Some(applicability) = self
            .context
            .trait_selection()
            .exact_candidate_applicability(&goal, trait_impl)
        {
            return Ok(applicability);
        }

        let candidate = TraitCandidate {
            trait_impl,
            subst: InferenceSubstitution::from_substitution(subst),
            applicability: TraitApplicability::Yes,
            table: InferenceTable::new(),
        };
        let outcome = self.select_candidate_with_header(&goal, candidate, header)?;
        let applicability = match outcome {
            SemanticOutcome::Available(selection) => {
                self.context
                    .trait_selection()
                    .remember_exact_candidate_applicability(
                        &goal,
                        trait_impl,
                        selection.applicability,
                    );
                selection.applicability
            }
            SemanticOutcome::Rejected => {
                self.context
                    .trait_selection()
                    .remember_exact_candidate_applicability(
                        &goal,
                        trait_impl,
                        TraitApplicability::No,
                    );
                TraitApplicability::No
            }
            SemanticOutcome::Unavailable => TraitApplicability::Maybe,
        };
        Ok(applicability)
    }

    /// Turn a native header match into semantic evidence by proving every remaining condition.
    fn select_candidate(
        &self,
        goal: &TraitGoal,
        candidate: TraitCandidate,
    ) -> Result<SemanticOutcome<TraitSelection>, I::Error> {
        let Some(header) = self.context.trait_selection().impl_header_with(
            self.context.item_paths(),
            self.context.item_paths(),
            candidate.trait_impl.impl_ref,
        )?
        else {
            return Ok(SemanticOutcome::Unavailable);
        };
        self.select_candidate_with_header(goal, candidate, &header)
    }

    /// Prove a candidate whose canonical header is already available to the caller.
    fn select_candidate_with_header(
        &self,
        goal: &TraitGoal,
        candidate: TraitCandidate,
        header: &crate::ImplHeader,
    ) -> Result<SemanticOutcome<TraitSelection>, I::Error> {
        let TraitCandidate {
            trait_impl,
            subst,
            mut applicability,
            mut table,
        } = candidate;

        if header.clauses.is_empty() {
            crate::profile::metric::PREDICATE_FREE_CANDIDATES.inc();
        } else {
            let predicate_applicability = self.context.trait_selection().prove_impl_bounds(
                self.context.item_paths(),
                self.context.crate_items(),
                &header.clauses,
                &subst,
                &table,
            )?;
            match predicate_applicability {
                ChalkOutcome::Proven(()) => {}
                ChalkOutcome::Ambiguous(_) => {
                    applicability = applicability.and(TraitApplicability::Maybe);
                }
                ChalkOutcome::NoSolution => return Ok(SemanticOutcome::Rejected),
                ChalkOutcome::Unsupported | ChalkOutcome::Exhausted => {
                    return Ok(SemanticOutcome::Unavailable);
                }
            };
        }

        match self.apply_assoc_type_constraints(
            goal,
            trait_impl.impl_ref,
            &subst,
            &mut table,
            &mut applicability,
        )? {
            SemanticOutcome::Available(()) => {}
            SemanticOutcome::Rejected => return Ok(SemanticOutcome::Rejected),
            SemanticOutcome::Unavailable => return Ok(SemanticOutcome::Unavailable),
        }

        Ok(SemanticOutcome::Available(TraitSelection {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }

    fn apply_assoc_type_constraints(
        &self,
        goal: &TraitGoal,
        impl_ref: ImplRef,
        subst: &InferenceSubstitution,
        table: &mut InferenceTable,
        applicability: &mut TraitApplicability,
    ) -> Result<SemanticOutcome<()>, I::Error> {
        if !goal.has_assoc_type_constraints() {
            return Ok(SemanticOutcome::Available(()));
        }

        let projection_goal = goal.without_assoc_type_constraints();
        for constraint in goal.assoc_type_constraints() {
            let Some(alias_data) = self
                .context
                .crate_items()
                .items()
                .type_alias_data(constraint.associated_ty)?
            else {
                return Ok(SemanticOutcome::Rejected);
            };
            let projection = self.context.trait_selection().normalize_assoc_type(
                self.context.item_paths(),
                self.context.crate_items(),
                &projection_goal,
                alias_data.name.as_str(),
                Some((impl_ref, subst)),
                table,
            )?;
            let projection = match projection {
                ChalkOutcome::Proven(projection) => projection,
                ChalkOutcome::Ambiguous(Some(projection)) => projection,
                ChalkOutcome::NoSolution => return Ok(SemanticOutcome::Rejected),
                ChalkOutcome::Ambiguous(None)
                | ChalkOutcome::Unsupported
                | ChalkOutcome::Exhausted => return Ok(SemanticOutcome::Unavailable),
            };

            let (projection_ty, mut projection_table) =
                self.normalize_ty(&projection.ty, &projection.table)?;
            if projection_table
                .try_unify(&projection_ty, constraint.ty)
                .is_err()
            {
                return Ok(SemanticOutcome::Rejected);
            }
            *table = projection_table;
            *applicability = applicability.and(projection.applicability);
        }

        Ok(SemanticOutcome::Available(()))
    }
}

#[cfg(test)]
mod tests;
