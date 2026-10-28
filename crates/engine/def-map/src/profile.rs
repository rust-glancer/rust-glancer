//! Metrics emitted while building DefMaps.
//!
//! Most metrics are standalone counters or timers declared below. Import resolution also records
//! one table row per collection run, so its schema and the row construction live together here.
//! This keeps profiling details out of import collection and makes column changes update one
//! small area.

use rg_ir_model::CrateRef;
use rg_profile::{
    ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, ProfileMeasurement,
    ProfileReport, ProfileReportSort, declare_metrics,
};

const BY_COUNT: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::CountDescending),
    limit: Some(20),
};
const BY_DURATION: ProfileReport = ProfileReport {
    sort: Some(ProfileReportSort::TotalDurationDescending),
    limit: Some(20),
};

const IMPORT_RESOLUTION_RUN_COLUMN_COUNT: usize = 9;

static IMPORT_RESOLUTION_RUN_COLUMNS: [ProfileCheckpointColumn;
    IMPORT_RESOLUTION_RUN_COLUMN_COUNT] = [
    ProfileCheckpointColumn::count("evaluated_packages", "packages"),
    ProfileCheckpointColumn::count("evaluated_crates", "crates"),
    ProfileCheckpointColumn::count("evaluated_modules", "modules"),
    ProfileCheckpointColumn::count("changed_packages", "pkg changed"),
    ProfileCheckpointColumn::count("changed_crates", "crate changed"),
    ProfileCheckpointColumn::count("changed_modules", "mod changed"),
    ProfileCheckpointColumn::count("imports_evaluated", "imports"),
    ProfileCheckpointColumn::count("glob_imports_evaluated", "globs"),
    ProfileCheckpointColumn::count("glob_bindings_emitted", "glob bindings"),
];

/// Values shown in one import-resolution checkpoint row.
///
/// “Evaluated” counts modules whose imports or glob updates were processed. “Changed” counts
/// distinct modules with a binding update during this run. The remaining fields count path queries
/// and glob bindings forwarded; unchanged partial answers still contribute to the query count.
pub(crate) struct ImportResolutionMetrics {
    pub(crate) evaluated_packages: usize,
    pub(crate) evaluated_crates: usize,
    pub(crate) evaluated_modules: usize,
    pub(crate) changed_packages: usize,
    pub(crate) changed_crates: usize,
    pub(crate) changed_modules: usize,
    pub(crate) imports_evaluated: usize,
    pub(crate) glob_imports_evaluated: usize,
    pub(crate) glob_bindings_emitted: usize,
}

/// Turn one collection run's counts into the checkpoint columns declared above.
///
/// The column and value arrays share `IMPORT_RESOLUTION_RUN_COLUMN_COUNT`. If a column is added or
/// removed, the compiler requires the value list to be updated as well.
pub(crate) fn record_import_resolution_run(
    krate: CrateRef,
    run_ordinal: usize,
    metrics: ImportResolutionMetrics,
) {
    let counts: [usize; IMPORT_RESOLUTION_RUN_COLUMN_COUNT] = [
        metrics.evaluated_packages,
        metrics.evaluated_crates,
        metrics.evaluated_modules,
        metrics.changed_packages,
        metrics.changed_crates,
        metrics.changed_modules,
        metrics.imports_evaluated,
        metrics.glob_imports_evaluated,
        metrics.glob_bindings_emitted,
    ];
    let values = IMPORT_RESOLUTION_RUN_COLUMNS
        .iter()
        .zip(counts)
        .map(|(column, count)| {
            ProfileCheckpointValue::new(column.key, ProfileMeasurement::count(count))
        })
        .collect();

    metric::IMPORT_RESOLUTION_RUN_CHECKPOINTS.record(
        format!(
            "crate {}:{} run {run_ordinal}",
            krate.package.0, krate.crate_id.0
        ),
        values,
    );
}

declare_metrics! {
    pub(crate) mod metric {
        scope "def_map.construction" {
            /// Number of fixed-point rounds required to build all module scopes.
            counter ROUNDS = "rounds";
            /// Number of import collection calls between macro batches.
            counter IMPORT_RESOLUTION_RUNS = "import_resolution.runs";
            /// Per-run import work and the part of the project whose resulting scopes changed.
            checkpoint IMPORT_RESOLUTION_RUN_CHECKPOINTS = "import_resolution.run_checkpoints" [columns &super::IMPORT_RESOLUTION_RUN_COLUMNS, title "Import resolution runs"];
            /// Number of glob updates stopped by the propagation-depth safeguard.
            counter IMPORT_GLOB_LIMIT_HITS = "import_resolution.glob_limit_hits";
            /// Number of macro-expansion passes performed during scope construction.
            counter EXPANSION_PASSES = "expansion_passes";
            /// Maximum number of macro-expansion passes allowed for one crate.
            gauge EXPANSION_PASS_LIMIT = "expansion_pass_limit" [Count];
            /// Whether scope construction stopped because the macro-expansion pass limit was reached.
            gauge EXPANSION_PASS_LIMIT_REACHED = "expansion_pass_limit_reached" [None];

            /// Time spent resolving import scopes during scope construction.
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
