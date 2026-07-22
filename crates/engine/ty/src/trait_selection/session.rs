//! Ownership and cache lifetimes for one trait-selection use-site crate.
//!
//! Crate-semantic state is shared by every query for the use-site. A session adds one inference
//! cache whose answers may contain body-owned variables and closure identities.
//!
//! `TraitSelectionSession::new` starts both layers. `fresh_inference_scope` and `for_body` keep the
//! crate layer and replace only the inference layer. Cloning any session shares all of the state it
//! already owns; a clone does not accidentally start another solver program or inference cache.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

use rg_def_map::DefMapSource;
use rg_ir_model::{CrateRef, ImplRef, TraitApplicability, TraitDefRef, TraitImplRef, TypeAliasRef};
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};

use super::chalk::{ChalkInferenceCache, ChalkOutcome, ChalkTraitSolver};
use super::matcher::{TraitImplCandidateIndex, TraitSelfHead};
use super::{AssocProjectionResult, TraitGoal, TraitSelection};
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::signature::impl_header_with;
use crate::{Clause, ItemPathQuery, TypePathResolver};

/// Cross-body part of a cached selection.
///
/// The trial inference table is intentionally absent. Stable goals do not solve body variables,
/// so a cache hit attaches the caller's own table instead of retaining one from an earlier query.
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

type SharedTraitImplCandidateIndex = Arc<Mutex<Option<TraitImplCandidateIndex>>>;
type TraitImplCandidateIndexes = Mutex<HashMap<TraitDefRef, SharedTraitImplCandidateIndex>>;
type ExactCandidateApplicabilities =
    Mutex<HashMap<TraitImplRef, HashMap<TraitGoal, TraitApplicability>>>;

/// Crate-semantic solver state shared by every session for one use-site crate.
///
/// Everything here is safe to keep after a body finishes. The separate inference cache on
/// `TraitSelectionSession` is the only place allowed to retain answers containing body variables
/// or closure identities.
struct TraitSelectionShared {
    use_site: CrateRef,
    /// Growing Chalk program plus solver forests for body-independent goals.
    solver: ChalkTraitSolver,
    /// Canonical headers lowered once and reused by candidate indexing and proof.
    impl_headers: Mutex<HashMap<ImplRef, Option<crate::ImplHeader>>>,
    /// Per-trait indexes that narrow visible impls by the outer shape of `Self`.
    trait_impl_candidates: TraitImplCandidateIndexes,
    /// Unique whole-goal selections whose inputs contain no body-owned identity.
    strict_selections: Mutex<HashMap<TraitGoal, ExpectedUnique<CachedTraitSelection>>>,
    /// Proof classifications for an already-selected impl and stable instantiated goal.
    exact_candidate_applicabilities: ExactCandidateApplicabilities,
}

impl TraitSelectionShared {
    fn new(use_site: CrateRef) -> Self {
        Self {
            use_site,
            solver: ChalkTraitSolver::new(),
            impl_headers: Mutex::new(HashMap::new()),
            trait_impl_candidates: Mutex::new(HashMap::new()),
            strict_selections: Mutex::new(HashMap::new()),
            exact_candidate_applicabilities: Mutex::new(HashMap::new()),
        }
    }
}

/// Reusable solver session for trait-selection probes with the same visible items.
///
/// Chalk program lowering follows every trait, impl, and opaque bound reachable from a goal, so its
/// cost is much larger than checking one candidate header. The use-site crate is part of the
/// session identity because different crates may see different impl universes for the same goal.
/// Cloning a session keeps its current inference-scoped Chalk cache too. `for_body` replaces only
/// that cache, so one body's fixed-point rounds can reuse local answers without retaining them for
/// unrelated bodies; the semantic program, candidate indexes, and stable answers remain shared.
#[derive(Clone)]
pub struct TraitSelectionSession {
    shared: Arc<TraitSelectionShared>,
    inference_cache: Arc<ChalkInferenceCache>,
}

impl fmt::Debug for TraitSelectionSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraitSelectionSession")
            .field("use_site", &self.shared.use_site)
            .finish_non_exhaustive()
    }
}

impl TraitSelectionSession {
    pub fn new(use_site: CrateRef) -> Self {
        Self {
            shared: Arc::new(TraitSelectionShared::new(use_site)),
            inference_cache: Arc::new(ChalkInferenceCache::new()),
        }
    }

    pub fn use_site(&self) -> CrateRef {
        self.shared.use_site
    }

    /// Keep crate-semantic solver state while starting an independent inference scope.
    ///
    /// This is the safe handoff between separate build or query operations. Chalk's program,
    /// canonical impl headers, candidate indexes, and stable answers remain shared, while answers
    /// containing inference variables or body identities stay with the operation that created
    /// them.
    pub fn fresh_inference_scope(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            inference_cache: Arc::new(ChalkInferenceCache::new()),
        }
    }

    /// Create one body-owned inference scope over the same crate-semantic caches.
    ///
    /// Closure identities and live inference variables cannot produce reusable crate-wide Chalk
    /// answers. Clones of the returned session share those answers during this body's fixed point,
    /// and dropping the session releases them before the next body starts.
    pub fn for_body(&self, body: rg_ir_model::BodyRef) -> Self {
        assert_eq!(
            body.crate_ref, self.shared.use_site,
            "body trait-selection scope must use the session crate"
        );
        self.fresh_inference_scope()
    }

    /// Prove one conjunction using this session's crate program and inference-scope cache.
    ///
    /// The solver chooses its stable or inference-scoped forest from the clauses themselves. This
    /// wrapper is what makes a `for_body` session carry local answers through fixed-point rounds.
    pub(crate) fn prove_clauses<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<InferenceTable>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.shared.solver.prove_clauses(
            item_paths,
            crate_items,
            lookup_index,
            self,
            &self.inference_cache,
            clauses,
            table,
        )
    }

    /// Normalize an associated type without losing the caller's inference-variable identities.
    ///
    /// Stable projections can reuse crate-wide answers. A projection involving closure or body
    /// variables uses this session's inference cache and disappears when the body session drops.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn normalize_assoc_type<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        goal: &TraitGoal,
        associated_ty: TypeAliasRef,
        selected_impl: Option<(ImplRef, &InferenceSubstitution)>,
        table: &InferenceTable,
    ) -> Result<ChalkOutcome<AssocProjectionResult>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.shared.solver.normalize_assoc_type(
            item_paths,
            crate_items,
            lookup_index,
            self,
            &self.inference_cache,
            goal,
            associated_ty,
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
            .shared
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
            .shared
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

    /// Narrow one trait's visible impls to headers with this outer `Self` shape.
    ///
    /// The first request lowers all visible headers for the trait and builds the index. Later
    /// requests reuse it, while blanket impls and other headless headers remain fallback entries.
    pub(crate) fn indexed_trait_impl_candidates<'query, D, I>(
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
            .shared
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

    /// Materialize definitions reachable from candidate predicates before proving candidates.
    ///
    /// Candidate selection checks matching impls one at a time. Extending the shared program here
    /// keeps that work outside the repeated solver loop.
    pub(crate) fn prepare_trait_impl_predicates<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
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
        self.shared
            .solver
            .prepare_clauses(item_paths, crate_items, lookup_index, self, &clauses)
    }

    /// Reattach the caller's table to a cached selection for a stable whole goal.
    ///
    /// Callers check `TraitGoal::is_cache_stable` before using this cache. Its stored payload has no
    /// trial table because a stable goal cannot carry body inference evidence.
    pub(crate) fn strict_selection(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Option<ExpectedUnique<TraitSelection>> {
        self.shared
            .strict_selections
            .lock()
            .expect("strict trait-selection cache lock should not be poisoned")
            .get(goal)
            .cloned()
            .map(|selection| selection.map(|selection| selection.with_table(table.clone())))
    }

    /// Cache a stable whole-goal result without retaining its trial table.
    pub(crate) fn remember_strict_selection(
        &self,
        goal: TraitGoal,
        selection: &ExpectedUnique<TraitSelection>,
    ) {
        let selection = selection.clone().map(CachedTraitSelection::from_selection);
        self.shared
            .strict_selections
            .lock()
            .expect("strict trait-selection cache lock should not be poisoned")
            .insert(goal, selection);
    }

    /// Read the stable proof classification for one already-matched impl.
    ///
    /// Body-owned goals deliberately miss this cache even if another body happened to use the
    /// same numeric inference-variable IDs.
    pub(crate) fn exact_candidate_applicability(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
    ) -> Option<TraitApplicability> {
        if !goal.is_cache_stable() {
            return None;
        }
        self.shared
            .exact_candidate_applicabilities
            .lock()
            .expect("exact trait-candidate cache lock should not be poisoned")
            .get(&trait_impl)?
            .get(goal)
            .copied()
    }

    /// Cache one exact impl classification only when its goal is body-independent.
    pub(crate) fn remember_exact_candidate_applicability(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        applicability: TraitApplicability,
    ) {
        if !goal.is_cache_stable() {
            return;
        }
        self.shared
            .exact_candidate_applicabilities
            .lock()
            .expect("exact trait-candidate cache lock should not be poisoned")
            .entry(trait_impl)
            .or_default()
            .insert(goal.clone(), applicability);
    }
}
