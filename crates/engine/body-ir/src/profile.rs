//! Profile descriptors for body inference and resolution.

use rg_profile::{ProfileDescriptor, declare_metrics};

declare_metrics! {
    pub(crate) mod metric {
        scope "body_ir.resolution" {
            /// Expressions whose syntax-directed inference rule was evaluated.
            counter EXPRESSION_VISITS = "expression_visits";
            /// Pattern nodes whose expected type was projected into bindings.
            counter PATTERN_VISITS = "pattern_visits";
            /// Attempts to select or refine a call signature.
            counter CALL_ATTEMPTS = "call_attempts";
            /// Attempts to project a field or index from its receiver.
            counter PROJECTION_ATTEMPTS = "projection_attempts";
            /// Selected-call predicate conjunctions submitted for inference evidence.
            counter OBLIGATION_ATTEMPTS = "obligation_attempts";
            /// Pending operations retried after their live inputs changed.
            counter DEFERRED_RETRIES = "deferred_retries";
            /// Expression subtrees stopped at the recursion safety limit.
            counter RECURSION_EXHAUSTIONS = "recursion_exhaustions";
            /// Bodies whose pending work reached its safety limit.
            counter DEFERRED_EXHAUSTIONS = "deferred_exhaustions";
        }
        scope "body_ir.lookup" {
            /// Lexical trait-scope questions served from one body-owned cache.
            counter TRAIT_SCOPE_CACHE_HITS = "trait_scopes.hits";
            /// Distinct body scopes whose visible traits were collected.
            counter TRAIT_SCOPE_CACHE_MISSES = "trait_scopes.misses";
            /// Trait declaration surfaces reused after lexical filtering in one body.
            counter TRAIT_SURFACE_CACHE_HITS = "trait_surfaces.hits";
            /// Distinct scope-and-surface pairs filtered for one body.
            counter TRAIT_SURFACE_CACHE_MISSES = "trait_surfaces.misses";
            /// Named extension-method probes skipped because no in-scope trait declares the name.
            counter EMPTY_TRAIT_EXTENSION_SURFACES = "trait_surfaces.empty_extension_probes";
            /// Trait-method misses served from one body-owned inference-aware cache.
            counter TRAIT_METHOD_MISS_CACHE_HITS = "trait_method_misses.hits";
            /// Distinct trait-method misses retained for one body.
            counter TRAIT_METHOD_MISS_CACHE_ENTRIES = "trait_method_misses.entries";
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
