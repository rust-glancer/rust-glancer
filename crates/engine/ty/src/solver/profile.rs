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
    pub slice_bytes: u64,
    pub outcomes: HashMap<&'static str, u64>,
    pub unavailable: HashMap<&'static str, u64>,
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
            (metric::SOLVER_SLICE_BYTES, self.slice_bytes),
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
    }
}
