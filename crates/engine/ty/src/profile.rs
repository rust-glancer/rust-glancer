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
            /// Declaration-part requests served from the operation's working cache.
            counter SOLVER_DECLARATION_HITS = "declarations.hits";
            /// Declaration-part requests served from the lexical context's owned cache.
            counter SOLVER_SHARED_DECLARATION_HITS = "declarations.shared_hits";
            /// Declaration parts read or lowered from semantic source storage.
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
            /// Requests to construct nonempty solver slices, summed across operations.
            counter SOLVER_SLICE_REQUESTS = "slices.requests";
            /// Nonempty slice requests grouped by element kind.
            keyed_counter SOLVER_SLICE_REQUESTS_BY_KIND = "slices.requests_by_kind" [report super::BY_COUNT, title "Solver slice requests"];
            /// Nonempty slice requests served without allocation, grouped by element kind.
            keyed_counter SOLVER_SLICE_HITS_BY_KIND = "slices.hits_by_kind" [report super::BY_COUNT, title "Solver slice reuse"];
            /// Slice payload bytes allocated across operations, grouped by element kind.
            keyed_counter SOLVER_SLICE_BYTES_BY_KIND = "slices.bytes_by_kind" [report super::BY_COUNT, title "Solver slice payload bytes"];
            /// Unique slices retained at operation exit, summed across operations.
            counter SOLVER_SLICE_ENTRIES = "slices.entries_total";
            /// Usable slice-table capacity at operation exit, summed across operations.
            counter SOLVER_SLICE_CAPACITY = "slices.capacity_total";
            /// Usable slice-table capacity times entry size; excludes control bytes and allocator overhead.
            counter SOLVER_SLICE_CAPACITY_BYTES = "slices.capacity_bytes_total";
            /// Slice payload bytes allocated across all operation arenas, not peak live memory.
            counter SOLVER_SLICE_BYTES = "arena.slice_bytes";
            /// Arena bytes reserved at operation exit, including metadata, summed across operations.
            counter SOLVER_ARENA_RESERVED_BYTES = "arena.reserved_bytes_total";
            /// Type, constant, and predicate nodes retained at operation exit, summed across operations.
            counter SOLVER_NODE_ENTRIES = "nodes.entries_total";
            /// Usable node-table capacity at operation exit, summed across operations.
            counter SOLVER_NODE_CAPACITY = "nodes.capacity_total";
            /// Usable node-table capacity times entry size; excludes control bytes and allocator overhead.
            counter SOLVER_NODE_CAPACITY_BYTES = "nodes.capacity_bytes_total";
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
