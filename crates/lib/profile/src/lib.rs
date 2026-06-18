//! Lightweight scoped profiling for rust-glancer internals.
//!
//! The crate separates the static profiling vocabulary from runtime collection. Instrumentation
//! call sites record by path, while descriptors decide which selector enables that path and how the
//! resulting value should be interpreted by report renderers.

mod descriptor;
mod filter;
mod macros;
mod metric;
mod registry;
mod runtime;
mod snapshot;

pub mod test_support;

pub use self::{
    descriptor::{
        ProfileCheckpointColumn, ProfileDescriptor, ProfileInstrumentKind, ProfilePathError,
        ProfileReport, ProfileReportSort, ProfileUnit, validate_profile_key, validate_profile_path,
    },
    filter::{ProfileFilter, ProfileFilterParseError},
    macros::{
        checkpoint, declare_metrics, increment_counter, increment_keyed_counter, record_duration,
        record_gauge, record_keyed_duration, timer,
    },
    metric::{
        CheckpointMetric, CounterMetric, DurationMetric, GaugeMetric, KeyedCounterMetric,
        KeyedDurationMetric, MemorySnapshotMetric,
    },
    registry::{ProfileFilterValidationError, ProfileRegistry, ProfileRegistryError},
    runtime::{
        ProfileInitializeError, ProfileRun, ProfileRunStartError, ProfileTimer, duration_enabled,
        initialize, memory_snapshot_enabled, record_checkpoint, record_duration, record_gauge,
        record_keyed_counter, record_keyed_duration, record_memory_snapshot, timer,
    },
    snapshot::{
        ProfileCheckpoint, ProfileCheckpointValue, ProfileEntry, ProfileKeyedCounter,
        ProfileKeyedDuration, ProfileMeasurement, ProfileMemoryRecord, ProfileMemorySnapshot,
        ProfileSnapshot, ProfileValue,
    },
};

pub fn record_counter(path: &'static str, amount: u64) {
    runtime::record_counter(path, amount);
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;

    static CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[
        ProfileCheckpointColumn::bytes("retained_bytes", "retained"),
        ProfileCheckpointColumn::count("packages", "packages"),
    ];

    declare_metrics! {
        pub(super) mod test_metric {
            scope "def_map.macros" {
                counter MACRO_CALLS_SEEN = "calls.seen";
                gauge PENDING_CALLS = "pending_calls" [Count];
            }
            scope "def_map.macros.by_name" {
                keyed_counter UNRESOLVED_BY_NAME = "unresolved";
                keyed_duration EXPANSION_BY_NAME = "expansion";
            }
            scope "def_map.finalize" {
                duration RESOLVE_IMPORT_SCOPES = "resolve_import_scopes";
            }
            scope "project.build" {
                checkpoint CHECKPOINTS = "checkpoints" [columns super::CHECKPOINT_COLUMNS];
            }
            scope "project.build.def_map" {
                memory_snapshot DEF_MAP_MEMORY = "memory" [title "after def-map"];
            }
        }
    }

    #[test]
    fn scoped_run_collects_enabled_metrics() {
        let run = test_support::ProfileTest::start(
            test_metric::descriptors(),
            "def_map.macros.by_name,def_map.finalize,project.build",
        );

        test_metric::MACRO_CALLS_SEEN.inc();
        test_metric::PENDING_CALLS.record_count(4);
        test_metric::UNRESOLVED_BY_NAME.inc("make_item");
        test_metric::RESOLVE_IMPORT_SCOPES.record(Duration::from_millis(7));
        test_metric::EXPANSION_BY_NAME.record("make_item", Duration::from_millis(3));
        test_metric::CHECKPOINTS.record(
            "after def-map",
            vec![
                ProfileCheckpointValue::new("retained_bytes", ProfileMeasurement::bytes(512)),
                ProfileCheckpointValue::new("packages", ProfileMeasurement::count(2)),
            ],
        );

        let snapshot = run.finish();

        snapshot.assert_counter_with_message(
            test_metric::MACRO_CALLS_SEEN,
            1,
            "the broad macro selector should include macro summary counters",
        );
        snapshot.assert_gauge_count_with_message(
            test_metric::PENDING_CALLS,
            4,
            "gauges should keep the latest value under their registered path",
        );
        snapshot.assert_keyed_counter_with_message(
            test_metric::UNRESOLVED_BY_NAME,
            "make_item",
            1,
            "the by-name selector should include keyed macro tables",
        );
        assert_eq!(
            snapshot
                .inner()
                .duration(test_metric::RESOLVE_IMPORT_SCOPES.path()),
            Some(Duration::from_millis(7)),
            "durations should accumulate under their registered path"
        );
        assert_eq!(
            snapshot
                .inner()
                .keyed_duration(test_metric::EXPANSION_BY_NAME.path(), "make_item")
                .map(|duration| duration.total),
            Some(Duration::from_millis(3)),
            "keyed duration aggregates should preserve total elapsed time"
        );
        let checkpoints = snapshot
            .inner()
            .checkpoints(test_metric::CHECKPOINTS.path())
            .expect("checkpoint stream should be present");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].label, "after def-map");
    }

    #[test]
    fn profile_run_collects_selected_memory_snapshots() {
        let run =
            test_support::ProfileTest::start(test_metric::descriptors(), "project.build.def_map");

        if test_metric::DEF_MAP_MEMORY.is_enabled() {
            test_metric::DEF_MAP_MEMORY.record(ProfileMemorySnapshot::new(
                64,
                vec![ProfileMemoryRecord::new(
                    "build.def_map",
                    "DefMapDb",
                    "heap",
                    64,
                )],
            ));
        }

        let snapshot = run.finish();
        let memory = snapshot
            .inner()
            .memory_snapshot(test_metric::DEF_MAP_MEMORY.path())
            .expect("selected memory snapshot should be recorded");

        assert_eq!(memory.retained_bytes, 64);
        assert_eq!(memory.records[0].path, "build.def_map");
    }

    #[test]
    fn inverted_filter_does_not_enable_more_detailed_scopes() {
        let run = test_support::ProfileTest::start(test_metric::descriptors(), "def_map.macros");

        test_metric::MACRO_CALLS_SEEN.inc();
        test_metric::UNRESOLVED_BY_NAME.inc("make_item");

        let snapshot = run.finish();

        snapshot.assert_counter(test_metric::MACRO_CALLS_SEEN, 1);
        assert_eq!(
            snapshot
                .inner()
                .keyed_counter(test_metric::UNRESOLVED_BY_NAME.path(), "make_item"),
            None,
            "selecting def_map.macros should not implicitly enable def_map.macros.by_name"
        );
    }

    #[test]
    #[should_panic(expected = "profile path `def_map.macros.calls.missing` is not registered")]
    fn active_run_panics_for_unknown_profile_path() {
        let _run = test_support::ProfileTest::start(test_metric::descriptors(), "def_map.macros");

        increment_counter!("def_map.macros.calls.missing");
    }

    #[test]
    fn disabled_recording_has_no_runtime_requirement() {
        increment_counter!("this.path.is.not.registered");
    }

    #[test]
    fn timers_record_elapsed_time_on_drop() {
        let run = test_support::ProfileTest::start(test_metric::descriptors(), "def_map.finalize");

        {
            let _timer = test_metric::RESOLVE_IMPORT_SCOPES.start_timer();
            thread::sleep(Duration::from_millis(1));
        }

        let elapsed = run
            .finish()
            .inner()
            .duration(test_metric::RESOLVE_IMPORT_SCOPES.path())
            .expect("timer should record one duration");
        assert!(elapsed > Duration::ZERO);
    }
}
