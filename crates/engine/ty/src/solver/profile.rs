//! Aggregate hot-loop counters locally, then publish them when their operation is released.
//!
//! A large body can evaluate the same family of goals many times. Recording each visit through
//! the shared profiler would add contention precisely when we are trying to measure that work.

use rustc_type_ir::data_structures::HashMap;

use crate::profile::metric;

#[derive(Default)]
pub(crate) struct SolverProfile {
    pub declaration_hits: u64,
    pub shared_declaration_hits: u64,
    pub declaration_loads: u64,
    pub fulfillments: u64,
    pub pending_visits: u64,
    pub evaluations: u64,
    pub fast_path_evaluations: u64,
    pub unavailable_reuses: u64,
    pub impl_candidates: u64,
    pub probes: u64,
    pub slice_requests: u64,
    pub slice_bytes: u64,
    pub slice_entries: u64,
    pub slice_capacity: u64,
    pub slice_capacity_bytes: u64,
    pub parameter_slice_requests: u64,
    pub parameter_slice_bytes: u64,
    pub arena_reserved_bytes: u64,
    pub node_entries: u64,
    pub node_capacity: u64,
    pub node_capacity_bytes: u64,
    pub outcomes: HashMap<&'static str, u64>,
    pub unavailable: HashMap<&'static str, u64>,
}

impl SolverProfile {
    pub fn record_node_table<K, V>(&mut self, table: &HashMap<K, V>) {
        self.node_entries += table.len() as u64;
        self.node_capacity += table.capacity() as u64;
        // Capacity counts usable entries, not raw buckets. This measures entry storage without
        // pretending to include the hash table's control bytes or allocator overhead.
        self.node_capacity_bytes += (table.capacity() * std::mem::size_of::<(K, V)>()) as u64;
    }
}

impl Drop for SolverProfile {
    fn drop(&mut self) {
        for (metric, count) in [
            (metric::SOLVER_DECLARATION_HITS, self.declaration_hits),
            (
                metric::SOLVER_SHARED_DECLARATION_HITS,
                self.shared_declaration_hits,
            ),
            (metric::SOLVER_DECLARATION_LOADS, self.declaration_loads),
            (metric::SOLVER_FULFILLMENTS, self.fulfillments),
            (metric::SOLVER_PENDING_VISITS, self.pending_visits),
            (metric::SOLVER_EVALUATIONS, self.evaluations),
            (
                metric::SOLVER_FAST_PATH_EVALUATIONS,
                self.fast_path_evaluations,
            ),
            (metric::SOLVER_UNAVAILABLE_REUSES, self.unavailable_reuses),
            (metric::SOLVER_IMPL_CANDIDATES, self.impl_candidates),
            (metric::SOLVER_PROBES, self.probes),
            (
                metric::SOLVER_SLICE_REQUESTS,
                self.slice_requests + self.parameter_slice_requests,
            ),
            (
                metric::SOLVER_SLICE_BYTES,
                self.slice_bytes + self.parameter_slice_bytes,
            ),
            (metric::SOLVER_SLICE_ENTRIES, self.slice_entries),
            (metric::SOLVER_SLICE_CAPACITY, self.slice_capacity),
            (
                metric::SOLVER_SLICE_CAPACITY_BYTES,
                self.slice_capacity_bytes,
            ),
            (
                metric::SOLVER_ARENA_RESERVED_BYTES,
                self.arena_reserved_bytes,
            ),
            (metric::SOLVER_NODE_ENTRIES, self.node_entries),
            (metric::SOLVER_NODE_CAPACITY, self.node_capacity),
            (metric::SOLVER_NODE_CAPACITY_BYTES, self.node_capacity_bytes),
        ] {
            if count != 0 {
                metric.add(count);
            }
        }
        for (&outcome, &count) in &self.outcomes {
            metric::SOLVER_OUTCOMES.add(outcome, count);
        }
        for (&reason, &count) in &self.unavailable {
            metric::SOLVER_UNAVAILABLE.add(reason, count);
        }
        if self.parameter_slice_requests != 0 {
            metric::SOLVER_SLICE_REQUESTS_BY_KIND.add("parameters", self.parameter_slice_requests);
            metric::SOLVER_SLICE_BYTES_BY_KIND.add("parameters", self.parameter_slice_bytes);
        }
    }
}
