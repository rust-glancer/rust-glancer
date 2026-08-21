//! Profile descriptors for body inference and resolution.

use rg_profile::{ProfileDescriptor, declare_metrics};

declare_metrics! {
    pub(crate) mod metric {
        scope "body_ir.resolution" {
            /// Bodies that retained semantic progress until the fixed-point safety limit.
            counter FIXED_POINT_EXHAUSTIONS = "fixed_point_exhaustions";
        }
        scope "body_ir.lookup" {
            /// Distinct ordered dependency sets that allocated lookup-result storage.
            counter DEPENDENCY_CACHE_CONSTRUCTIONS = "dependency_caches.constructed";
            /// Use-site queries that reused lookup-result storage for a dependency set.
            counter DEPENDENCY_CACHE_REUSES = "dependency_caches.reused";
            /// Dependency lookup keys served from previously computed results.
            counter DEPENDENCY_RESULT_HITS = "dependency_results.hits";
            /// Dependency lookup keys computed from persisted indexes for the first time.
            counter DEPENDENCY_RESULT_MISSES = "dependency_results.misses";
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
