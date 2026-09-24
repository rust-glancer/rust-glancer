//! Profile descriptor vocabulary for type-layer integrations.

use rg_profile::{ProfileDescriptor, ProfileReport, ProfileReportSort, declare_metrics};

const BY_COUNT: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::CountDescending),
    limit: Some(20),
};
declare_metrics! {
    pub(crate) mod metric {
        scope "ty.lowering" {
            /// Recursive source-type lowering stopped at a semantic cycle or depth boundary.
            keyed_counter TYPE_LOWERING_LIMIT_EXHAUSTIONS = "limit_exhaustions" [report super::BY_COUNT, title "Type-lowering limit exhaustions"];
        }
        scope "ty.solver" {
            /// Independent solver arenas created for bodies and standalone queries.
            counter SOLVER_OPERATIONS = "operations";
            /// Declaration requests served from an operation's owned source cache.
            counter SOLVER_DECLARATION_HITS = "declarations.hits";
            /// Declaration requests served by another operation in the same lexical context.
            counter SOLVER_SHARED_DECLARATION_HITS = "declarations.shared_hits";
            /// Declarations lowered from semantic source storage.
            counter SOLVER_DECLARATION_LOADS = "declarations.loads";
            /// Calls that revisit a table's pending obligations.
            counter SOLVER_FULFILLMENTS = "fulfillment.calls";
            /// Pending obligations inspected across fulfillment rounds.
            counter SOLVER_PENDING_VISITS = "fulfillment.pending_visits";
            /// Root goals passed to the compiler solver.
            counter SOLVER_EVALUATIONS = "goals.evaluated";
            /// Pending goals answered by the delegate without entering a root evaluation.
            counter SOLVER_FAST_PATH_EVALUATIONS = "goals.fast_path";
            /// Unavailable obligations left pending while all their inference inputs are unchanged.
            counter SOLVER_UNAVAILABLE_REUSES = "goals.unavailable_reuses";
            /// Root evaluations grouped by their outcome.
            keyed_counter SOLVER_OUTCOMES = "goals.outcomes" [report super::BY_COUNT, title "Solver goal outcomes"];
            /// Unavailable evaluations grouped by the missing prerequisite.
            keyed_counter SOLVER_UNAVAILABLE = "unavailable" [report super::BY_COUNT, title "Solver unavailable reasons"];
            /// Impl identities enumerated for compiler solver callbacks.
            counter SOLVER_IMPL_CANDIDATES = "impl_candidates";
            /// Body candidate searches that clone the inference context.
            counter SOLVER_PROBES = "candidate_probes";
            /// Generic and predicate slices allocated in operation arenas.
            counter SOLVER_SLICE_BYTES = "arena.slice_bytes";
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
