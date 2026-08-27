//! Ownership and cache lifetimes for trait selection.
//!
//! Trait selection reuses work at three different boundaries:
//!
//! 1. `TraitSelectionDeclarationCache` belongs to one immutable semantic snapshot. Sessions for
//!    different use-site crates may share it because canonical crate declaration types do not
//!    contain visibility decisions or solver answers.
//! 2. `TraitSelectionShared` belongs to one use-site crate. It owns that crate's growing Chalk
//!    program, stable solver forests, and body-origin impl headers. Saved candidate discovery is
//!    delegated to the operation-scoped Semantic IR lookup query.
//! 3. `TraitSelectionInferenceScope` is attached to a session for one inference operation. It may
//!    contain live variables, closure identities, or receiver matches observed while resolving one
//!    body, so `fresh_inference_scope` and `for_body` replace only this layer.
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
use super::matcher::TraitSelfHead;
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

type ExactCandidateApplicabilities =
    Mutex<HashMap<TraitImplRef, HashMap<TraitGoal, TraitApplicability>>>;

/// One canonical impl header matched against an exact, uncanonicalized receiver `Ty`.
///
/// For `impl<T> Trait for [T]` and receiver `[User]`, this retains the canonical header plus
/// `T = User`. Impl parameters that do not occur in `Self` are intentionally still absent; the
/// later semantic probe gives those parameters fresh variables in the caller's table. Receiver
/// variables remain in the substitution by identity; this value neither reads nor snapshots their
/// inference-table solutions.
#[derive(Clone)]
pub(crate) struct CachedImplSelfMatch {
    pub(crate) header: Arc<crate::ImplHeader>,
    pub(crate) subst: crate::Substitution,
    pub(crate) applicability: TraitApplicability,
}

type ImplSelfMatches = Mutex<HashMap<Ty, HashMap<ImplRef, Option<CachedImplSelfMatch>>>>;

/// Broad impl lists retained only while one inference scope can reuse their charged work.
///
/// A receiver such as a generic `T` or an unnormalized alias has no stable self head. Once the
/// surrounding lookup has selected `DisplayLike`, it must conservatively inspect every visible
/// `DisplayLike` impl. This cache makes later fixed-point rounds reuse that already-charged list.
type BroadTraitImpls = Mutex<HashMap<TraitDefRef, UniqueVec<TraitImplRef>>>;

// One body can legitimately ask many cheap questions, while a pathological body must not turn
// thousands of individually bounded operations into unbounded aggregate work. This allowance is
// deliberately much larger than one settled Chalk goal and is shared by every clone of the
// body-owned session.
const BODY_TRAIT_WORK_LIMIT: usize = 65_536;

/// Deterministic work charged to one body-owned trait-selection session.
///
/// These are accounting labels, not stages of trait proof. For example, opening a broad trait lane
/// charges `BroadCandidateSet` once for its declaration count, then checking each retained header
/// charges `CandidateProbe` as that work actually happens.
#[derive(Clone, Copy)]
pub(super) enum TraitWorkKind {
    /// Declarations admitted when the receiver has no stable outer head.
    BroadCandidateSet,
    /// One impl header compared or semantically proved as a candidate.
    CandidateProbe,
    /// One declaration added to the growing Chalk program.
    ProgramDefinition,
    /// One alias or associated-type normalization step.
    NormalizationStep,
    /// One bounded unit of Chalk solver work.
    SolverQuantum,
}

impl TraitWorkKind {
    fn label(self) -> &'static str {
        match self {
            Self::BroadCandidateSet => "body_work.broad_candidate_set",
            Self::CandidateProbe => "body_work.candidate_probe",
            Self::ProgramDefinition => "body_work.program_definition",
            Self::NormalizationStep => "body_work.normalization_step",
            Self::SolverQuantum => "body_work.solver_quantum",
        }
    }
}

/// The boundary that made a best-effort trait query stop.
///
/// `Aggregate` means the body spent its shared allowance across otherwise bounded operations.
/// `NormalizationDepth` means one recursive alias/projection chain reached its own depth limit.
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
///
/// Body resolution recreates adapters and revisits expressions, so a per-call limit would merely
/// reset on every retry. This tracker makes exhaustion sticky for the complete body operation and
/// records whether its single fail-soft warning has already been emitted.
struct TraitWorkTracker {
    body: Option<BodyRef>,
    limit: Option<usize>,
    remaining: AtomicUsize,
    exhausted: AtomicBool,
    reported: AtomicBool,
}

impl TraitWorkTracker {
    fn unbounded() -> Self {
        Self {
            body: None,
            limit: None,
            remaining: AtomicUsize::new(usize::MAX),
            exhausted: AtomicBool::new(false),
            reported: AtomicBool::new(false),
        }
    }

    fn for_body(body: BodyRef, limit: usize) -> Self {
        Self {
            body: Some(body),
            limit: Some(limit),
            remaining: AtomicUsize::new(limit),
            exhausted: AtomicBool::new(false),
            reported: AtomicBool::new(false),
        }
    }

    /// Reserve work before starting an operation so concurrent session clones cannot overspend.
    fn consume(&self, amount: usize) -> bool {
        if self.limit.is_none() {
            return true;
        }
        if self.exhausted.load(Ordering::Relaxed) {
            return false;
        }
        if amount == 0 {
            return true;
        }

        let mut remaining = self.remaining.load(Ordering::Relaxed);
        loop {
            if remaining < amount {
                // Once one operation cannot fit, this body has crossed its aggregate fail-soft
                // boundary. Later fixed-point retries must not rebuild the same candidate set only
                // to rediscover that boundary.
                self.exhausted.store(true, Ordering::Relaxed);
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

    fn is_exhausted(&self) -> bool {
        self.exhausted.load(Ordering::Relaxed)
    }

    fn mark_reported(&self) -> bool {
        !self.reported.swap(true, Ordering::Relaxed)
    }
}

/// Crate-semantic solver state shared by one use-site session and its inference scopes.
///
/// Everything here is safe to keep for the rest of the snapshot after one body finishes. The
/// separate [`TraitSelectionInferenceScope`] is the only place allowed to retain answers containing
/// body variables or closure identities.
struct TraitSelectionShared {
    use_site: CrateRef,
    /// Use-site-independent crate declarations shared across sessions over the same snapshot.
    declarations: TraitSelectionDeclarationCache,
    /// Growing Chalk program plus solver forests for body-independent goals.
    solver: ChalkTraitSolver,
    /// Body-origin headers cannot enter the snapshot declaration cache because lexical views may
    /// differ between requests. Keep them within the session that owns their body source.
    body_impl_headers: Mutex<HashMap<ImplRef, Option<Arc<crate::ImplHeader>>>>,
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
            strict_selections: Mutex::new(HashMap::new()),
            exact_candidate_applicabilities: Mutex::new(HashMap::new()),
        }
    }
}

/// Caches and work accounting that must not outlive one inference operation.
///
/// A body can clone its trait-selection session across query adapters and fixed-point rounds, but
/// all of those clones belong to the same inference scope. Keeping this layer behind one `Arc`
/// makes that lifetime explicit and prevents one cache from accidentally surviving after its
/// inference variables, receiver matches, or work allowance have been discarded.
///
/// For example, repeated attempts to resolve `value.convert()` may reuse all of the following:
///
/// - Chalk's answer for a goal containing this body's inference variables;
/// - the lowered header for `impl<T> Convert for Vec<T>`;
/// - the match `Vec<T>` against the current `Vec<u16>` receiver, including `T = u16`;
/// - a broad list used while the receiver was an alias or type parameter;
/// - the work already charged for those attempts.
///
/// Dropping the body session releases the whole group together. Stable declaration and solver data
/// remains in [`TraitSelectionShared`] for the next inference operation.
struct TraitSelectionInferenceScope {
    chalk: ChalkInferenceCache,
    /// Front the snapshot-wide declaration table for headers repeatedly used by one inference
    /// scope. This avoids contending on the large shared identity map during fixed-point retries.
    impl_headers: Mutex<HashMap<ImplRef, Option<Arc<crate::ImplHeader>>>>,
    /// Repeated fixed-point rounds often compare the same raw receiver with the same conservative
    /// fallback impls. Retain both positive and negative matches against canonical headers.
    impl_self_matches: ImplSelfMatches,
    /// Parameters and unresolved aliases have no stable receiver head, so they must consider every
    /// visible impl of an already-selected trait. Retain that broad list after its first charge.
    broad_trait_impls: BroadTraitImpls,
    work: TraitWorkTracker,
}

impl TraitSelectionInferenceScope {
    fn new(work: TraitWorkTracker) -> Self {
        Self {
            chalk: ChalkInferenceCache::new(),
            impl_headers: Mutex::new(HashMap::new()),
            impl_self_matches: Mutex::new(HashMap::new()),
            broad_trait_impls: Mutex::new(HashMap::new()),
            work,
        }
    }
}

/// Reusable solver session for trait-selection probes with the same visible items.
///
/// Chalk program lowering follows every trait, impl, and opaque bound reachable from a goal, so its
/// cost is much larger than checking one candidate header. The use-site crate is part of the
/// session identity because different crates may see different impl universes for the same goal.
/// Cloning a session keeps its current inference scope too. `for_body` replaces that complete
/// layer, so one body's fixed-point rounds can reuse local answers without retaining variables,
/// receiver matches, or work accounting for unrelated bodies; the semantic program and stable
/// answers remain shared.
#[derive(Clone)]
pub struct TraitSelectionSession {
    shared: Arc<TraitSelectionShared>,
    inference_scope: Arc<TraitSelectionInferenceScope>,
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
            inference_scope: Arc::new(TraitSelectionInferenceScope::new(
                TraitWorkTracker::unbounded(),
            )),
        }
    }

    pub fn use_site(&self) -> CrateRef {
        self.shared.use_site
    }

    /// Keep crate-semantic solver state while starting an independent inference scope.
    ///
    /// This is the safe handoff between independent inference operations over the same snapshot.
    /// Snapshot crate declarations, Chalk's program, and stable answers remain shared, while
    /// answers containing inference variables or body identities stay with the operation that
    /// created them.
    pub fn fresh_inference_scope(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            inference_scope: Arc::new(TraitSelectionInferenceScope::new(
                TraitWorkTracker::unbounded(),
            )),
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
            inference_scope: Arc::new(TraitSelectionInferenceScope::new(
                TraitWorkTracker::for_body(body, BODY_TRAIT_WORK_LIMIT),
            )),
        }
    }

    /// Charge deterministic work to this inference scope before starting an expensive step.
    ///
    /// Standalone editor queries remain governed by their operation-specific limits. Body-owned
    /// sessions additionally share one aggregate allowance across fixed-point rounds and clones.
    pub(super) fn consume_work(&self, kind: TraitWorkKind, amount: usize) -> bool {
        if self.inference_scope.work.consume(amount) {
            return true;
        }
        self.report_limit(
            TraitWorkLimit::Aggregate(kind),
            self.inference_scope.work.limit,
        );
        false
    }

    /// Report one fail-soft boundary per inference scope without flooding a pathological body.
    pub(super) fn report_limit(&self, kind: TraitWorkLimit, limit: Option<usize>) {
        if !self.inference_scope.work.mark_reported() {
            return;
        }
        crate::profile::metric::WORK_LIMIT_EXHAUSTIONS.inc(kind.label());
        tracing::warn!(
            use_site = ?self.shared.use_site,
            body = ?self.inference_scope.work.body,
            limit_kind = kind.label(),
            limit,
            "trait selection stopped at a work limit; affected results remain unavailable"
        );
    }

    #[cfg(test)]
    pub(super) fn with_work_limit(mut self, limit: usize) -> Self {
        self.inference_scope = Arc::new(TraitSelectionInferenceScope::new(
            TraitWorkTracker::for_body(
                BodyRef {
                    crate_ref: self.shared.use_site,
                    body: rg_ir_model::BodyId(0),
                },
                limit,
            ),
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
            &self.inference_scope.chalk,
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
            &self.inference_scope.chalk,
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
        if let Some(header) = self
            .inference_scope
            .impl_headers
            .lock()
            .expect("inference-scope impl-header cache lock should not be poisoned")
            .get(&impl_ref)
            .cloned()
        {
            return Ok(header);
        }

        let header = if impl_ref.origin.as_crate_ref().is_some() {
            self.shared.declarations.impl_header(impl_ref, || {
                lower_impl_header(item_paths, resolver, impl_ref)
            })?
        } else if let Some(header) = self
            .shared
            .body_impl_headers
            .lock()
            .expect("body impl-header cache lock should not be poisoned")
            .get(&impl_ref)
            .cloned()
        {
            header
        } else {
            let header = lower_impl_header(item_paths, resolver, impl_ref)?.map(Arc::new);
            self.remember_body_impl_header(impl_ref, header.clone());
            header
        };

        // An error returns above and therefore never enters either cache. Parallel successful
        // loads may race, but a late conservative absence must not replace an available header.
        let mut headers = self
            .inference_scope
            .impl_headers
            .lock()
            .expect("inference-scope impl-header cache lock should not be poisoned");
        let published = if header.is_some() {
            headers.insert(impl_ref, header.clone());
            header
        } else {
            headers.entry(impl_ref).or_insert(None).clone()
        };
        Ok(published)
    }

    /// Reuse a raw-receiver match against a canonical impl header within one inference scope.
    ///
    /// The key is the exact `Ty` value and does not consult the caller's inference table. Solving a
    /// variable in a separate table therefore does not change this raw operation's input; only a
    /// stronger `Ty` representation produces a different key. The later semantic proof still
    /// receives the live table.
    ///
    /// `None` is useful because fallback indexes include many impls that structurally do not match
    /// one receiver. The inference-scope lifetime prevents raw variable and closure identities from
    /// escaping their owning operation. Errors return before publication so a later fixed-point
    /// round can retry the same declaration.
    pub(crate) fn impl_self_match_or_try_init<E>(
        &self,
        receiver_ty: &Ty,
        impl_ref: ImplRef,
        load: impl FnOnce() -> Result<Option<CachedImplSelfMatch>, E>,
    ) -> Result<Option<CachedImplSelfMatch>, E> {
        if let Some(value) = self
            .inference_scope
            .impl_self_matches
            .lock()
            .expect("impl self-match cache lock should not be poisoned")
            .get(receiver_ty)
            .and_then(|by_impl| by_impl.get(&impl_ref))
            .cloned()
        {
            return Ok(value);
        }

        let value = load()?;
        let mut matches = self
            .inference_scope
            .impl_self_matches
            .lock()
            .expect("impl self-match cache lock should not be poisoned");
        let published = matches
            .entry(receiver_ty.clone())
            .or_default()
            .entry(impl_ref)
            .or_insert_with(|| value.clone())
            .clone();
        Ok(published)
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
    ///
    /// The surrounding query has already selected the trait from an item name, a bound, or a
    /// completion surface. This method only chooses the receiver lane for that trait:
    ///
    /// ```text
    /// `u32` or `Vec<u8>`   -> direct matching head plus conservative fallbacks
    /// closure/function item -> fallbacks only (their identity cannot be written in an impl)
    /// parameter or alias    -> every visible impl of this already-selected trait
    /// `?T` or unknown       -> no candidates; impls must not infer a receiver from nothing
    /// ```
    ///
    /// The returned declarations are still candidates. Exact header matching and trait proof happen
    /// afterwards. `None` means this inference scope exhausted its fail-soft work allowance;
    /// `Some(empty)` is an ordinary lookup result.
    pub(crate) fn trait_impl_candidates_for_ty(
        &self,
        item_lookup: &ItemLookupQuery<'_>,
        trait_ref: TraitDefRef,
        self_ty: &Ty,
    ) -> Option<UniqueVec<TraitImplRef>> {
        if self.inference_scope.work.is_exhausted() {
            return None;
        }

        // A receiver with no established shape cannot use the impl universe as an inverse source
        // of inference. Parameters and aliases are established types, but have no stable outer
        // head, so they retain every impl of this one already-selected trait.
        if matches!(self_ty, Ty::InferVar { .. } | Ty::Unknown) {
            return Some(UniqueVec::new());
        }
        if let Some(self_head) = TraitSelfHead::from_ty(self_ty) {
            return Some(
                item_lookup
                    .trait_impl_candidates_for_self_head(trait_ref, self_head.impl_lookup_head())
                    .unwrap_or_default(),
            );
        }

        if let Some(visible_impls) = self
            .inference_scope
            .broad_trait_impls
            .lock()
            .expect("broad trait-impl cache lock should not be poisoned")
            .get(&trait_ref)
            .cloned()
        {
            return Some(visible_impls);
        }

        let visible_impls = item_lookup
            .trait_impls_for_trait(trait_ref)
            .unwrap_or_default();
        let mut broad_trait_impls = self
            .inference_scope
            .broad_trait_impls
            .lock()
            .expect("broad trait-impl cache lock should not be poisoned");
        // Another clone may have populated the same inference-scope cache while declaration
        // lookup ran. Its reservation owns the work charge, so reuse that completed result.
        if let Some(visible_impls) = broad_trait_impls.get(&trait_ref).cloned() {
            return Some(visible_impls);
        }
        if !self.consume_work(TraitWorkKind::BroadCandidateSet, visible_impls.len()) {
            return None;
        }
        broad_trait_impls.insert(trait_ref, visible_impls.clone());
        Some(visible_impls)
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
