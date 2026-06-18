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
            counter ROUNDS = "rounds";
            counter EXPANSION_PASSES = "expansion_passes";
            gauge EXPANSION_PASS_LIMIT = "expansion_pass_limit" [Count];
            gauge EXPANSION_PASS_LIMIT_REACHED = "expansion_pass_limit_reached" [None];

            duration TIMING_RESOLVE_IMPORT_SCOPES = "timings.resolve_import_scopes";
            duration TIMING_COLLECT_EXPANSION_ATTEMPTS = "timings.collect_expansion_attempts";
            duration TIMING_APPLY_EXPANSION_ATTEMPTS = "timings.apply_expansion_attempts";
            duration TIMING_COMPILE_MACROS = "timings.compile_macros";
            duration TIMING_EXPAND_MACROS = "timings.expand_macros";
            duration TIMING_PARSE_GENERATED_SOURCES = "timings.parse_generated_sources";
            duration TIMING_COLLECT_GENERATED_ITEMS = "timings.collect_generated_items";
        }

        scope "def_map.macros" {
            counter MACRO_CALLS_SEEN = "calls.seen";
            counter MACRO_CALLS_RESOLVED = "calls.resolved";
            counter MACRO_CALLS_UNRESOLVED = "calls.unresolved";
            counter MACRO_CALLS_SKIPPED = "calls.skipped";
            counter MACRO_CALLS_SKIPPED_BY_LIMIT = "calls.skipped_by_limit";
            counter MACRO_CALLS_EXPANDED = "calls.expanded";
            counter MACRO_CALLS_FAILED = "calls.failed";

            counter MACRO_COMPILE_ATTEMPTS = "compile.attempts";
            counter MACRO_COMPILE_CACHE_HITS = "compile.cache_hits";
            counter MACRO_COMPILE_FAILURES = "compile.failures";
            counter MACRO_EXPAND_ATTEMPTS = "expand.attempts";
            counter MACRO_EXPAND_CACHE_HITS = "expand.cache_hits";
            counter MACRO_EXPAND_FAILURES = "expand.failures";

            counter GENERATED_SOURCES_PARSED = "generated.sources_parsed";
            counter GENERATED_SOURCE_PARSE_FAILURES = "generated.source_parse_failures";
            counter GENERATED_ITEMS_SEEN = "generated.items_seen";
        }

        scope "def_map.macros.by_name" {
            keyed_counter FAILED_COMPILE_BY_NAME = "failures.compile" [report super::BY_COUNT];
            keyed_counter FAILED_EXPAND_BY_NAME = "failures.expand" [report super::BY_COUNT];
            keyed_counter FAILED_PARSE_BY_NAME = "failures.parse" [report super::BY_COUNT];
            keyed_counter UNRESOLVED_BY_NAME = "unresolved" [report super::BY_COUNT];
            keyed_duration EXPANSION_BY_NAME = "expansion" [report super::BY_DURATION];
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
