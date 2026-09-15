//! Aggregate limits on trait work shared by one inference scope.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rg_ir_model::BodyRef;

// One body can legitimately ask many cheap questions, while a pathological body must not turn
// thousands of individually bounded operations into unbounded aggregate work. This allowance is
// deliberately much larger than one settled Chalk goal and is shared by every clone of the
// body-owned session.
pub(crate) const BODY_TRAIT_WORK_LIMIT: usize = 65_536;

/// Deterministic work charged to one body-owned trait-selection session.
///
/// These are accounting labels, not stages of trait proof. For example, opening a broad trait lane
/// charges `BroadCandidateSet` once for its declaration count, then checking each retained header
/// charges `CandidateProbe` as that work actually happens.
#[derive(Clone, Copy)]
pub(crate) enum TraitWorkKind {
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
pub(crate) enum TraitWorkLimit {
    Aggregate(TraitWorkKind),
    NormalizationDepth,
}

impl TraitWorkLimit {
    pub(crate) fn label(self) -> &'static str {
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
pub(crate) struct TraitWorkTracker {
    pub(crate) body: Option<BodyRef>,
    pub(crate) limit: Option<usize>,
    remaining: AtomicUsize,
    exhausted: AtomicBool,
    reported: AtomicBool,
}

impl TraitWorkTracker {
    pub(crate) fn unbounded() -> Self {
        Self {
            body: None,
            limit: None,
            remaining: AtomicUsize::new(usize::MAX),
            exhausted: AtomicBool::new(false),
            reported: AtomicBool::new(false),
        }
    }

    pub(crate) fn for_body(body: BodyRef, limit: usize) -> Self {
        Self {
            body: Some(body),
            limit: Some(limit),
            remaining: AtomicUsize::new(limit),
            exhausted: AtomicBool::new(false),
            reported: AtomicBool::new(false),
        }
    }

    /// Reserve work before starting an operation so concurrent session clones cannot overspend.
    pub(crate) fn consume(&self, amount: usize) -> bool {
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

    pub(crate) fn is_exhausted(&self) -> bool {
        self.exhausted.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_reported(&self) -> bool {
        !self.reported.swap(true, Ordering::Relaxed)
    }
}
