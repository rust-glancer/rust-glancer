//! Profile descriptor vocabulary for type-layer integrations.

use rg_profile::{ProfileDescriptor, ProfileReport, ProfileReportSort, declare_metrics};

const BY_COUNT: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::CountDescending),
    limit: Some(20),
};
const BY_DURATION: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::TotalDurationDescending),
    limit: Some(20),
};

declare_metrics! {
    pub(crate) mod metric {
        scope "ty.lowering" {
            /// Recursive source-type lowering stopped at a semantic cycle or depth boundary.
            keyed_counter TYPE_LOWERING_LIMIT_EXHAUSTIONS = "limit_exhaustions" [report super::BY_COUNT, title "Type-lowering limit exhaustions"];
        }
        scope "ty.trait_selection.chalk" {
            /// Impl candidates whose header matched and had no predicates left for Chalk to prove.
            counter PREDICATE_FREE_CANDIDATES = "candidates.predicate_free";
            /// Chalk programs built for trait-selection predicate solving.
            counter PROGRAM_BUILDS = "program.builds";
            /// Chalk program build time.
            duration PROGRAM_BUILD_TIME = "timings.program_build";
            /// Definitions discovered while constructing Chalk programs, grouped by kind.
            keyed_counter PROGRAM_DEFINITIONS_BY_KIND = "program.definitions" [report super::BY_COUNT, title "Chalk program definitions"];
            /// Declaration-cache lookups grouped by declaration kind and hit or miss.
            keyed_counter DECLARATION_CACHE_ACCESSES = "program.declaration_cache" [report super::BY_COUNT, title "Chalk declaration cache"];
            /// Chalk program construction time grouped by materialization phase.
            keyed_duration PROGRAM_BUILD_TIME_BY_PHASE = "timings.program_build_phase" [report super::BY_DURATION, title "Chalk program build phases"];
            /// Chalk predicate goals sent to the solver.
            counter SOLVER_GOALS = "solver.goals";
            /// Chalk predicate goals that produced an ambiguous answer.
            counter SOLVER_AMBIGUOUS_GOALS = "solver.goals.ambiguous";
            /// Bounded outcomes grouped by solver operation and result.
            keyed_counter SOLVER_GOAL_OUTCOMES = "solver.goal_outcomes" [report super::BY_COUNT, title "Chalk solver goal outcomes"];
            /// Solver goals grouped by the input shape that selected their work budget.
            keyed_counter SOLVER_GOAL_SHAPES = "solver.goal_shapes" [report super::BY_COUNT, title "Chalk solver goal shapes"];
            /// Repeated body-local goals served from a previous bounded decline.
            counter DECLINED_GOAL_REUSES = "solver.declined_goal_reuses";
            /// Body-owned aggregate work or recursive normalization stopped at a safety boundary.
            keyed_counter WORK_LIMIT_EXHAUSTIONS = "work_limit_exhaustions" [report super::BY_COUNT, title "Trait-selection work-limit exhaustions"];
            /// Associated projections instantiated directly from a selected native impl.
            counter NATIVE_ASSOC_PROJECTIONS = "native.assoc_projections";
            /// Candidate projection probes stopped at an actual recursive impl cycle.
            counter NATIVE_CANDIDATE_CYCLES = "native.candidate_cycles";
            /// Impl identities rejected before header loading because their crate cannot own an
            /// implementation for the fully known trait application.
            counter NATIVE_CANDIDATE_COHERENCE_SKIPS = "native.candidate_coherence_skips";
            /// Candidate projections left for one combined solver goal because the matching impl
            /// had predicates of its own.
            counter NATIVE_CANDIDATE_PREDICATE_DECLINES = "native.candidate_predicate_declines";
            /// Native projection shortcuts skipped because their goal was body-owned or lacked an
            /// indexable receiver head.
            counter NATIVE_CANDIDATE_UNSTABLE_DECLINES = "native.candidate_unstable_declines";
            /// Chalk predicate-solving time grouped by goal kind.
            keyed_duration SOLVER_GOAL_TIME_BY_KIND = "timings.solver_goal" [report super::BY_DURATION, title "Chalk solver goals"];
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
