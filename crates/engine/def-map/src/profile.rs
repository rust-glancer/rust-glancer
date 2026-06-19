//! Profile descriptor vocabulary for def-map construction.

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
        scope "def_map.finalization" {
            /// Number of fixpoint rounds required to finalize all def maps.
            counter ROUNDS = "rounds";
            /// Number of macro-expansion passes performed during finalization.
            counter EXPANSION_PASSES = "expansion_passes";
            /// Maximum number of macro-expansion passes allowed for one finalization run.
            gauge EXPANSION_PASS_LIMIT = "expansion_pass_limit" [Count];
            /// Whether finalization stopped because the macro-expansion pass limit was reached.
            gauge EXPANSION_PASS_LIMIT_REACHED = "expansion_pass_limit_reached" [None];

            /// Time spent resolving import scopes during finalization.
            duration TIMING_RESOLVE_IMPORT_SCOPES = "timings.resolve_import_scopes";
            /// Time spent collecting macro expansion attempts.
            duration TIMING_COLLECT_EXPANSION_ATTEMPTS = "timings.collect_expansion_attempts";
            /// Time spent applying collected macro expansion attempts.
            duration TIMING_APPLY_EXPANSION_ATTEMPTS = "timings.apply_expansion_attempts";
            /// Time spent compiling macro definitions.
            duration TIMING_COMPILE_MACROS = "timings.compile_macros";
            /// Time spent expanding macro calls.
            duration TIMING_EXPAND_MACROS = "timings.expand_macros";
            /// Time spent parsing generated macro expansion sources.
            duration TIMING_PARSE_GENERATED_SOURCES = "timings.parse_generated_sources";
            /// Time spent collecting items from generated macro expansion sources.
            duration TIMING_COLLECT_GENERATED_ITEMS = "timings.collect_generated_items";
        }

        scope "def_map.macros" {
            /// Macro calls encountered while building def maps.
            counter MACRO_CALLS_SEEN = "calls.seen";
            /// Macro calls whose macro definition was resolved.
            counter MACRO_CALLS_RESOLVED = "calls.resolved";
            /// Macro calls whose macro definition could not be resolved.
            counter MACRO_CALLS_UNRESOLVED = "calls.unresolved";
            /// Macro calls skipped before expansion.
            counter MACRO_CALLS_SKIPPED = "calls.skipped";
            /// Macro calls skipped because the expansion pass limit was reached.
            counter MACRO_CALLS_SKIPPED_BY_LIMIT = "calls.skipped_by_limit";
            /// Macro calls expanded successfully.
            counter MACRO_CALLS_EXPANDED = "calls.expanded";
            /// Macro calls whose expansion failed.
            counter MACRO_CALLS_FAILED = "calls.failed";

            /// Attempts to compile macro definitions.
            counter MACRO_COMPILE_ATTEMPTS = "compile.attempts";
            /// Macro definition compilations served from cache.
            counter MACRO_COMPILE_CACHE_HITS = "compile.cache_hits";
            /// Macro definition compilation failures.
            counter MACRO_COMPILE_FAILURES = "compile.failures";
            /// Attempts to expand macro calls.
            counter MACRO_EXPAND_ATTEMPTS = "expand.attempts";
            /// Macro expansions served from cache.
            counter MACRO_EXPAND_CACHE_HITS = "expand.cache_hits";
            /// Macro expansion failures.
            counter MACRO_EXPAND_FAILURES = "expand.failures";

            /// Generated macro expansion sources parsed successfully.
            counter GENERATED_SOURCES_PARSED = "generated.sources_parsed";
            /// Generated macro expansion sources that failed to parse.
            counter GENERATED_SOURCE_PARSE_FAILURES = "generated.source_parse_failures";
            /// Items collected from generated macro expansion sources.
            counter GENERATED_ITEMS_SEEN = "generated.items_seen";
        }

        scope "def_map.macros.by_name" {
            /// Macro definition compilation failures grouped by macro name.
            keyed_counter FAILED_COMPILE_BY_NAME = "failures.compile" [report super::BY_COUNT, title "Macro compilation failures"];
            /// Macro expansion failures grouped by macro name.
            keyed_counter FAILED_EXPAND_BY_NAME = "failures.expand" [report super::BY_COUNT, title "Macro expansion failures"];
            /// Generated-source parse failures grouped by macro name.
            keyed_counter FAILED_PARSE_BY_NAME = "failures.parse" [report super::BY_COUNT, title "Macro parsing failures"];
            /// Unresolved macro calls grouped by macro name.
            keyed_counter UNRESOLVED_BY_NAME = "unresolved" [report super::BY_COUNT, title "Unresolved macros"];
            /// Macro expansion time grouped by macro name.
            keyed_duration EXPANSION_BY_NAME = "expansion" [report super::BY_DURATION, title "Slowest macros to expand"];
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
