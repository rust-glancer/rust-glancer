//! Profile descriptor vocabulary for type-layer integrations.

use rg_profile::{ProfileDescriptor, ProfileReport, ProfileReportSort, declare_metrics};

const BY_DURATION: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::TotalDurationDescending),
    limit: Some(20),
};

declare_metrics! {
    pub(crate) mod metric {
        scope "ty.trait_selection.chalk" {
            /// Impl candidates whose header matched and had no predicates left for Chalk to prove.
            counter PREDICATE_FREE_CANDIDATES = "candidates.predicate_free";
            /// Chalk programs built for trait-selection predicate solving.
            counter PROGRAM_BUILDS = "program.builds";
            /// Chalk program build time.
            duration PROGRAM_BUILD_TIME = "timings.program_build";
            /// Chalk predicate goals sent to the solver.
            counter SOLVER_GOALS = "solver.goals";
            /// Chalk predicate goals that produced an ambiguous answer.
            counter SOLVER_AMBIGUOUS_GOALS = "solver.goals.ambiguous";
            /// Chalk predicate-solving time grouped by goal kind.
            keyed_duration SOLVER_GOAL_TIME_BY_KIND = "timings.solver_goal" [report super::BY_DURATION, title "Chalk solver goals"];
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
