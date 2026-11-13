//! Profile descriptors for body inference, resolution, and const evaluation.

use rg_profile::{ProfileDescriptor, declare_metrics};

declare_metrics! {
    pub(crate) mod metric {
        scope "body_ir.saved" {
            /// Outer owners computed by the saved-body provider; reuse does not increment this.
            counter OWNERS = "owners";
        }
        scope "body_ir.const_eval" {
            /// Body-local and current-root const and static initializers submitted for evaluation.
            counter LOCAL_CONST_INITIALIZERS = "local.initializers";
            /// Submitted initializers that produced a concrete value for their local ItemStore.
            counter LOCAL_CONST_KNOWN_VALUES = "local.known_values";
            /// Evaluation and publication in local stores; excludes ordinary body lowering.
            duration LOCAL_CONST_EVALUATION = "local.timings.evaluation";
        }
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

        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
