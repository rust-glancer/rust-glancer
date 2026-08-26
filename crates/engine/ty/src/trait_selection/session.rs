//! Ownership and cache lifetimes for trait selection.
//!
//! Trait selection reuses work at three different boundaries:
//!
//! 1. `TraitSelectionDeclarationCache` belongs to one immutable semantic snapshot. Sessions for
//!    different use-site crates may share it because canonical crate declaration types do not
//!    contain visibility decisions or solver answers.
//! 2. `TraitSelectionShared` belongs to one use-site crate. It owns that crate's visible candidate
//!    indexes, growing Chalk program, stable solver forests, and body-origin impl headers.
//! 3. `TraitSelectionSession` adds an inference cache. That cache may contain live variables and
//!    closure identities, so `fresh_inference_scope` and `for_body` replace only this layer.
//!
//! Cloning a session shares every layer already attached to it. `new` creates a standalone
//! declaration layer; `new_with_declaration_cache` joins the snapshot layer supplied by a project
//! build or query owner.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    BodyRef, CrateRef, FunctionRef, GenericDefRef, ImplRef, TraitApplicability, TraitDefRef,
    TraitImplRef, TypeAliasRef,
};
use rg_semantic_ir::{CrateItemQuery, ItemLookupQuery, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};

use super::chalk::{ChalkInferenceCache, ChalkOutcome, ChalkTraitSolver};
use super::declaration_cache::{OpaqueBounds, TraitSelectionDeclarationCache};
use super::matcher::{TraitImplCandidateIndex, TraitSelfHead};
use super::{AssocProjectionResult, TraitGoal, TraitSelection};
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::signature::impl_header_with as lower_impl_header;
use crate::{
    CallableSignature, Clause, ItemPathQuery, SemanticSignatureQuery, Ty, TypePathResolver,
};

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

    fn with_table(
        self,
        application: crate::TraitApplication,
        table: InferenceTable,
    ) -> TraitSelection {
        TraitSelection {
            trait_impl: self.trait_impl,
            application,
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

// One body can legitimately ask many cheap questions, while a pathological body must not turn
// thousands of individually bounded operations into unbounded aggregate work. This allowance is
// deliberately much larger than one settled Chalk goal and is shared by every clone of the
// body-owned session.
const BODY_TRAIT_WORK_LIMIT: usize = 65_536;

/// Deterministic work charged to one body-owned trait-selection session.
#[derive(Clone, Copy)]
pub(super) enum TraitWorkKind {
    CandidateIndex,
    CandidateProbe,
    ProgramDefinition,
    NormalizationStep,
    SolverQuantum,
}

impl TraitWorkKind {
    fn label(self) -> &'static str {
        match self {
            Self::CandidateIndex => "body_work.candidate_index",
            Self::CandidateProbe => "body_work.candidate_probe",
            Self::ProgramDefinition => "body_work.program_definition",
            Self::NormalizationStep => "body_work.normalization_step",
            Self::SolverQuantum => "body_work.solver_quantum",
        }
    }
}

/// The boundary that made a best-effort trait query stop.
#[derive(Clone, Copy)]
pub(super) enum TraitWorkLimit {
    Aggregate(TraitWorkKind),
    NormalizationDepth,
}

impl TraitWorkLimit {
    fn label(self) -> &'static str {
        match self {
            Self::Aggregate(kind) => kind.label(),
            Self::NormalizationDepth => "normalization_depth",
        }
    }
}

/// Work and reporting state shared by every clone of one inference scope.
struct TraitWorkTracker {
    body: Option<BodyRef>,
    limit: Option<usize>,
    remaining: AtomicUsize,
    reported: AtomicBool,
}

impl TraitWorkTracker {
    fn unbounded() -> Self {
        Self {
            body: None,
            limit: None,
            remaining: AtomicUsize::new(usize::MAX),
            reported: AtomicBool::new(false),
        }
    }

    fn for_body(body: BodyRef, limit: usize) -> Self {
        Self {
            body: Some(body),
            limit: Some(limit),
            remaining: AtomicUsize::new(limit),
            reported: AtomicBool::new(false),
        }
    }

    /// Reserve work before starting an operation so concurrent session clones cannot overspend.
    fn consume(&self, amount: usize) -> bool {
        if self.limit.is_none() || amount == 0 {
            return true;
        }

        let mut remaining = self.remaining.load(Ordering::Relaxed);
        loop {
            if remaining < amount {
                return false;
            }
            match self.remaining.compare_exchange_weak(
                remaining,
                remaining - amount,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => remaining = observed,
            }
        }
    }

    fn mark_reported(&self) -> bool {
        !self.reported.swap(true, Ordering::Relaxed)
    }
}

/// Crate-semantic solver state shared by one use-site session and its inference scopes.
///
/// Everything here is safe to keep for the rest of the snapshot after one body finishes. The
/// separate inference cache on `TraitSelectionSession` is the only place allowed to retain answers
/// containing body variables or closure identities.
struct TraitSelectionShared {
    use_site: CrateRef,
    /// Use-site-independent crate declarations shared across sessions over the same snapshot.
    declarations: TraitSelectionDeclarationCache,
    /// Growing Chalk program plus solver forests for body-independent goals.
    solver: ChalkTraitSolver,
    /// Body-origin headers cannot enter the snapshot declaration cache because lexical views may
    /// differ between requests. Keep them within the session that owns their body source.
    body_impl_headers: Mutex<HashMap<ImplRef, Option<Arc<crate::ImplHeader>>>>,
    /// Per-trait indexes that narrow visible impls by the outer shape of `Self`.
    trait_impl_candidates: TraitImplCandidateIndexes,
    /// Unique whole-goal selections whose inputs contain no body-owned identity.
    strict_selections: Mutex<HashMap<TraitGoal, ExpectedUnique<CachedTraitSelection>>>,
    /// Proof classifications for an already-selected impl and stable instantiated goal.
    exact_candidate_applicabilities: ExactCandidateApplicabilities,
}

impl TraitSelectionShared {
    fn new(use_site: CrateRef, declarations: TraitSelectionDeclarationCache) -> Self {
        Self {
            use_site,
            declarations,
            solver: ChalkTraitSolver::new(),
            body_impl_headers: Mutex::new(HashMap::new()),
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
    work: Arc<TraitWorkTracker>,
}

impl fmt::Debug for TraitSelectionSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraitSelectionSession")
            .field("use_site", &self.shared.use_site)
            .finish_non_exhaustive()
    }
}

impl TraitSelectionSession {
    /// Start a standalone use-site session with its own declaration cache.
    ///
    /// Use [`Self::new_with_declaration_cache`] when several use-site crates are analyzed from the
    /// same semantic snapshot and should share canonical crate declaration lowering.
    pub fn new(use_site: CrateRef) -> Self {
        Self::new_with_declaration_cache(use_site, TraitSelectionDeclarationCache::new())
    }

    /// Start one use-site solver while reusing canonical crate declarations from the same snapshot.
    ///
    /// Another use-site session given the same `declarations` handle shares only declaration-owned
    /// types. Visibility indexes, solver forests, stable answers, and inference caches start fresh
    /// for this use-site crate; clones of this returned session then share those layers normally.
    pub fn new_with_declaration_cache(
        use_site: CrateRef,
        declarations: TraitSelectionDeclarationCache,
    ) -> Self {
        Self {
            shared: Arc::new(TraitSelectionShared::new(use_site, declarations)),
            inference_cache: Arc::new(ChalkInferenceCache::new()),
            work: Arc::new(TraitWorkTracker::unbounded()),
        }
    }

    pub fn use_site(&self) -> CrateRef {
        self.shared.use_site
    }

    /// Keep crate-semantic solver state while starting an independent inference scope.
    ///
    /// This is the safe handoff between independent inference operations over the same snapshot.
    /// Snapshot crate declarations, Chalk's program, candidate indexes, and stable answers remain
    /// shared, while answers containing inference variables or body identities stay with the
    /// operation that created them.
    pub fn fresh_inference_scope(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            inference_cache: Arc::new(ChalkInferenceCache::new()),
            work: Arc::new(TraitWorkTracker::unbounded()),
        }
    }

    /// Create one body-owned inference scope over the same crate-semantic caches.
    ///
    /// Closure identities and live inference variables cannot produce reusable crate-wide Chalk
    /// answers. Clones of the returned session share those answers during this body's fixed point,
    /// and dropping the session releases them before the next body starts.
    pub fn for_body(&self, body: BodyRef) -> Self {
        assert_eq!(
            body.crate_ref, self.shared.use_site,
            "body trait-selection scope must use the session crate"
        );
        Self {
            shared: self.shared.clone(),
            inference_cache: Arc::new(ChalkInferenceCache::new()),
            work: Arc::new(TraitWorkTracker::for_body(body, BODY_TRAIT_WORK_LIMIT)),
        }
    }

    /// Charge deterministic work to this inference scope before starting an expensive step.
    ///
    /// Standalone editor queries remain governed by their operation-specific limits. Body-owned
    /// sessions additionally share one aggregate allowance across fixed-point rounds and clones.
    pub(super) fn consume_work(&self, kind: TraitWorkKind, amount: usize) -> bool {
        if self.work.consume(amount) {
            return true;
        }
        self.report_limit(TraitWorkLimit::Aggregate(kind), self.work.limit);
        false
    }

    /// Report one fail-soft boundary per inference scope without flooding a pathological body.
    pub(super) fn report_limit(&self, kind: TraitWorkLimit, limit: Option<usize>) {
        if !self.work.mark_reported() {
            return;
        }
        crate::profile::metric::WORK_LIMIT_EXHAUSTIONS.inc(kind.label());
        tracing::warn!(
            use_site = ?self.shared.use_site,
            body = ?self.work.body,
            limit_kind = kind.label(),
            limit,
            "trait selection stopped at a work limit; affected results remain unavailable"
        );
    }

    #[cfg(test)]
    pub(super) fn with_work_limit(mut self, limit: usize) -> Self {
        self.work = Arc::new(TraitWorkTracker::for_body(
            BodyRef {
                crate_ref: self.shared.use_site,
                body: rg_ir_model::BodyId(0),
            },
            limit,
        ));
        self
    }

    /// Prove one conjunction using this session's crate program and inference-scope cache.
    ///
    /// The solver chooses its stable or inference-scoped forest from the clauses themselves. This
    /// wrapper is what makes a `for_body` session carry local answers through fixed-point rounds.
    pub(crate) fn prove_clauses<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        item_lookup: &ItemLookupQuery<'_>,
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
            item_lookup,
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
        item_lookup: &ItemLookupQuery<'_>,
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
            item_lookup,
            self,
            &self.inference_cache,
            goal,
            associated_ty,
            selected_impl,
            table,
        )
    }

    /// Reuse an impl header without crossing the resolver boundary that produced it.
    ///
    /// A crate-origin impl has one canonical header for the semantic snapshot, so all use-site
    /// sessions may share its lowering. A body-origin impl can resolve through request-local items;
    /// its header therefore stays in the use-site session that owns that body source.
    pub(crate) fn impl_header_with<'query, D, I, R>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        resolver: &R,
        impl_ref: ImplRef,
    ) -> Result<Option<Arc<crate::ImplHeader>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
        R: TypePathResolver<Error = I::Error>,
    {
        if impl_ref.origin.as_crate_ref().is_some() {
            return self.shared.declarations.impl_header(impl_ref, || {
                lower_impl_header(item_paths, resolver, impl_ref)
            });
        }

        if let Some(header) = self
            .shared
            .body_impl_headers
            .lock()
            .expect("body impl-header cache lock should not be poisoned")
            .get(&impl_ref)
            .cloned()
        {
            return Ok(header);
        }

        let header = lower_impl_header(item_paths, resolver, impl_ref)?.map(Arc::new);
        self.remember_body_impl_header(impl_ref, header.clone());
        Ok(header)
    }

    /// Publish a body header without letting a later conservative miss erase a successful load.
    fn remember_body_impl_header(&self, impl_ref: ImplRef, header: Option<Arc<crate::ImplHeader>>) {
        let mut headers = self
            .shared
            .body_impl_headers
            .lock()
            .expect("body impl-header cache lock should not be poisoned");
        // Parallel misses can finish out of order. A successfully lowered header may replace a
        // conservative miss, while a late miss must not erase a header another worker found.
        if header.is_some() {
            headers.insert(impl_ref, header);
        } else {
            headers.entry(impl_ref).or_insert(None);
        }
    }

    /// Lower a trait's canonical `Self` and predicates at the lifetime allowed by its origin.
    ///
    /// Crate traits use the snapshot declaration cache. Body traits are lowered from the caller's
    /// body-aware item view and are not placed in that cross-use-site cache.
    pub(crate) fn trait_header_with<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<Option<Arc<crate::signature::TraitHeader>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if trait_ref.origin.as_crate_ref().is_some() {
            return self.shared.declarations.trait_header(trait_ref, || {
                SemanticSignatureQuery::trait_header_from(item_paths, trait_ref)
            });
        }
        Ok(SemanticSignatureQuery::trait_header_from(item_paths, trait_ref)?.map(Arc::new))
    }

    /// Lower the value behind a declaration such as `type Item = Vec<T>`.
    ///
    /// Crate aliases use the snapshot declaration cache. A body alias may name local items, so it
    /// is lowered from the caller's item view instead of being shared across use-site sessions.
    pub(crate) fn type_alias_ty_with<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        alias: TypeAliasRef,
    ) -> Result<Option<Arc<Ty>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if alias.origin.as_crate_ref().is_some() {
            return self.shared.declarations.type_alias_ty(alias, || {
                SemanticSignatureQuery::type_alias_ty_from(item_paths, alias)
            });
        }
        Ok(SemanticSignatureQuery::type_alias_ty_from(item_paths, alias)?.map(Arc::new))
    }

    /// Lower a function's canonical params, return type, and clauses.
    ///
    /// Crate functions use the snapshot declaration cache. Body functions are lowered through the
    /// body-aware item view instead of being shared across use-site sessions.
    pub(crate) fn function_signature_with<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        function: FunctionRef,
    ) -> Result<Option<Arc<CallableSignature>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if function.origin.as_crate_ref().is_some() {
            return self.shared.declarations.function_signature(function, || {
                SemanticSignatureQuery::function_from(item_paths, function)
            });
        }
        Ok(SemanticSignatureQuery::function_from(item_paths, function)?.map(Arc::new))
    }

    /// Lower every opaque identity and bound declared by one generic owner.
    ///
    /// For `fn items<T>() -> impl Iterator<Item = T>`, this keeps the opaque return identity beside
    /// its lowered `Iterator` bound. Crate owners use the snapshot cache; body owners are lowered
    /// from their local item view.
    pub(crate) fn opaque_bounds_for_owner_with<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        owner: GenericDefRef,
    ) -> Result<Arc<OpaqueBounds>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if owner.origin().as_crate_ref().is_some() {
            return self.shared.declarations.opaque_bounds(owner, || {
                SemanticSignatureQuery::opaque_bounds_for_owner_from(item_paths, owner)
            });
        }
        Ok(Arc::new(
            SemanticSignatureQuery::opaque_bounds_for_owner_from(item_paths, owner)?,
        ))
    }

    /// Narrow one trait's visible impls using the receiver's established outer shape.
    pub(crate) fn trait_impl_candidates_for_ty<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        item_lookup: &ItemLookupQuery<'_>,
        trait_ref: TraitDefRef,
        self_ty: &Ty,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // A receiver with no established shape cannot use the impl universe as an inverse source
        // of inference. Parameters and aliases are established types, but have no stable outer
        // head, so they retain every impl of this one already-selected trait.
        if matches!(self_ty, Ty::InferVar { .. } | Ty::Unknown) {
            return Ok(Some(UniqueVec::new()));
        }
        let Some(visible_impls) = item_lookup.trait_impls_for_trait(trait_ref) else {
            return Ok(Some(UniqueVec::new()));
        };
        if let Some(self_head) = TraitSelfHead::from_ty(self_ty) {
            return self.indexed_trait_impl_candidates(
                item_paths,
                trait_ref,
                visible_impls,
                self_head,
            );
        }

        let visible_impls = visible_impls.into_iter().collect::<Vec<_>>();
        if !self.consume_work(TraitWorkKind::CandidateIndex, visible_impls.len()) {
            return Ok(None);
        }
        Ok(Some(visible_impls.into_iter().collect()))
    }

    pub(crate) fn indexed_trait_impl_candidates<'query, D, I>(
        &self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
        visible_impls: impl IntoIterator<Item = TraitImplRef>,
        self_head: TraitSelfHead,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, I::Error>
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
            return Ok(Some(index.candidates(self_head)));
        }

        // Lower every visible header for this trait once, then answer all later receiver queries
        // from its semantic `Self` fingerprint. The per-trait lock makes initialization
        // single-flight without serializing indexes for unrelated traits.
        let visible_impls = visible_impls.into_iter().collect::<Vec<_>>();
        if !self.consume_work(TraitWorkKind::CandidateIndex, visible_impls.len()) {
            return Ok(None);
        }
        let mut built = TraitImplCandidateIndex::default();
        for trait_impl in visible_impls {
            let Some(header) =
                self.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
            else {
                continue;
            };
            built.push(trait_impl, &header);
        }

        let candidates = built.candidates(self_head);
        *index = Some(built);
        Ok(Some(candidates))
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
            .map(|selection| {
                selection
                    .map(|selection| selection.with_table(goal.application.clone(), table.clone()))
            })
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
