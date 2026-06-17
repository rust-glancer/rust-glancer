//! Lightweight scoped profiling for rust-glancer internals.
//!
//! The crate separates the static profiling vocabulary from runtime collection. Instrumentation
//! call sites record by path, while descriptors decide which selector enables that path and how the
//! resulting value should be interpreted by report renderers.

mod descriptor;
mod filter;
mod registry;
mod runtime;
mod snapshot;

pub use self::{
    descriptor::{
        ProfileCheckpointColumn, ProfileDescriptor, ProfileInstrumentKind, ProfilePathError,
        ProfileReport, ProfileReportSort, ProfileUnit, validate_profile_key, validate_profile_path,
    },
    filter::{ProfileFilter, ProfileFilterParseError},
    registry::{ProfileFilterValidationError, ProfileRegistry, ProfileRegistryError},
    runtime::{
        ProfileInitializeError, ProfileRun, ProfileRunStartError, ProfileTimer, initialize,
        record_checkpoint, record_duration, record_gauge, record_keyed_counter,
        record_keyed_duration, timer,
    },
    snapshot::{
        ProfileCheckpoint, ProfileCheckpointValue, ProfileEntry, ProfileKeyedCounter,
        ProfileKeyedDuration, ProfileMeasurement, ProfileSnapshot, ProfileValue,
    },
};

/// Increments a registered counter by one, or by the provided amount.
#[macro_export]
macro_rules! increment_counter {
    ($path:literal) => {
        $crate::record_counter($path, 1)
    };
    ($path:literal, $amount:expr) => {
        $crate::record_counter($path, $amount)
    };
}

/// Records a keyed counter increment.
#[macro_export]
macro_rules! increment_keyed_counter {
    ($path:literal, $key:expr) => {
        $crate::record_keyed_counter($path, $key, 1)
    };
    ($path:literal, $key:expr, $amount:expr) => {
        $crate::record_keyed_counter($path, $key, $amount)
    };
}

/// Adds elapsed time to a registered duration.
#[macro_export]
macro_rules! record_duration {
    ($path:literal, $duration:expr) => {
        $crate::record_duration($path, $duration)
    };
}

/// Records the latest value for a registered gauge.
#[macro_export]
macro_rules! record_gauge {
    ($path:literal, $value:expr) => {
        $crate::record_gauge($path, $value)
    };
}

/// Adds elapsed time to a keyed duration aggregate.
#[macro_export]
macro_rules! record_keyed_duration {
    ($path:literal, $key:expr, $duration:expr) => {
        $crate::record_keyed_duration($path, $key, $duration)
    };
}

/// Starts an RAII timer that records elapsed time when dropped.
#[macro_export]
macro_rules! timer {
    ($path:literal) => {
        $crate::timer($path)
    };
}

/// Appends a row to a checkpoint stream.
#[macro_export]
macro_rules! checkpoint {
    ($path:literal, $label:expr $(, $key:literal => $value:expr)* $(,)?) => {{
        let values = ::std::vec![
            $($crate::ProfileCheckpointValue::new($key, $value),)*
        ];
        $crate::record_checkpoint($path, $label, values)
    }};
}

pub fn record_counter(path: &'static str, amount: u64) {
    runtime::record_counter(path, amount);
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Mutex, MutexGuard},
        thread,
        time::Duration,
    };

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[
        ProfileCheckpointColumn::bytes("retained_bytes", "retained"),
        ProfileCheckpointColumn::count("packages", "packages"),
    ];

    fn test_lock() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn registry() -> ProfileRegistry {
        ProfileRegistry::new([
            ProfileDescriptor::counter("def_map.macros.calls.seen", "def_map.macros")
                .title("seen macro calls"),
            ProfileDescriptor::gauge(
                "def_map.macros.pending_calls",
                "def_map.macros",
                ProfileUnit::Count,
            ),
            ProfileDescriptor::keyed_counter(
                "def_map.macros.unresolved.by_name",
                "def_map.macros.by_name",
            ),
            ProfileDescriptor::duration(
                "def_map.finalize.resolve_import_scopes",
                "def_map.finalize",
            ),
            ProfileDescriptor::keyed_duration(
                "def_map.macros.expansion.by_name",
                "def_map.macros.by_name",
            ),
            ProfileDescriptor::checkpoint_stream("project.build.checkpoints", "project.build")
                .checkpoint_columns(CHECKPOINT_COLUMNS),
        ])
        .expect("test profile registry should be valid")
    }

    #[test]
    fn scoped_run_collects_enabled_metrics() {
        let _lock = test_lock();
        let run = ProfileRun::start_with_registry(
            registry(),
            ProfileFilter::parse("def_map.macros.by_name,def_map.finalize,project.build")
                .expect("filter should parse"),
        )
        .expect("profile run should start");

        increment_counter!("def_map.macros.calls.seen");
        record_gauge!("def_map.macros.pending_calls", ProfileMeasurement::count(4));
        increment_keyed_counter!("def_map.macros.unresolved.by_name", "make_item");
        record_duration!(
            "def_map.finalize.resolve_import_scopes",
            Duration::from_millis(7)
        );
        record_keyed_duration!(
            "def_map.macros.expansion.by_name",
            "make_item",
            Duration::from_millis(3)
        );
        checkpoint!(
            "project.build.checkpoints",
            "after def-map",
            "retained_bytes" => ProfileMeasurement::bytes(512),
            "packages" => ProfileMeasurement::count(2),
        );

        let snapshot = run.finish();

        assert_eq!(
            snapshot.counter("def_map.macros.calls.seen"),
            Some(1),
            "the broad macro selector should include macro summary counters"
        );
        assert_eq!(
            snapshot
                .entry("def_map.macros.pending_calls")
                .map(ProfileEntry::value),
            Some(&ProfileValue::Gauge(ProfileMeasurement::count(4))),
            "gauges should keep the latest value under their registered path"
        );
        assert_eq!(
            snapshot.keyed_counter("def_map.macros.unresolved.by_name", "make_item"),
            Some(1),
            "the by-name selector should include keyed macro tables"
        );
        assert_eq!(
            snapshot.duration("def_map.finalize.resolve_import_scopes"),
            Some(Duration::from_millis(7)),
            "durations should accumulate under their registered path"
        );
        assert_eq!(
            snapshot
                .keyed_duration("def_map.macros.expansion.by_name", "make_item")
                .map(|duration| duration.total),
            Some(Duration::from_millis(3)),
            "keyed duration aggregates should preserve total elapsed time"
        );
        let checkpoints = snapshot
            .checkpoints("project.build.checkpoints")
            .expect("checkpoint stream should be present");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].label, "after def-map");
    }

    #[test]
    fn inverted_filter_does_not_enable_more_detailed_scopes() {
        let _lock = test_lock();
        let run = ProfileRun::start_with_registry(
            registry(),
            ProfileFilter::parse("def_map.macros").expect("filter should parse"),
        )
        .expect("profile run should start");

        increment_counter!("def_map.macros.calls.seen");
        increment_keyed_counter!("def_map.macros.unresolved.by_name", "make_item");

        let snapshot = run.finish();

        assert_eq!(snapshot.counter("def_map.macros.calls.seen"), Some(1));
        assert_eq!(
            snapshot.keyed_counter("def_map.macros.unresolved.by_name", "make_item"),
            None,
            "selecting def_map.macros should not implicitly enable def_map.macros.by_name"
        );
    }

    #[test]
    #[should_panic(expected = "profile path `def_map.macros.calls.missing` is not registered")]
    fn active_run_panics_for_unknown_profile_path() {
        let _lock = test_lock();
        let _run = ProfileRun::start_with_registry(
            registry(),
            ProfileFilter::parse("def_map.macros").expect("filter should parse"),
        )
        .expect("profile run should start");

        increment_counter!("def_map.macros.calls.missing");
    }

    #[test]
    fn disabled_recording_has_no_runtime_requirement() {
        let _lock = test_lock();
        increment_counter!("this.path.is.not.registered");
    }

    #[test]
    fn timers_record_elapsed_time_on_drop() {
        let _lock = test_lock();
        let run = ProfileRun::start_with_registry(
            registry(),
            ProfileFilter::parse("def_map.finalize").expect("filter should parse"),
        )
        .expect("profile run should start");

        {
            let _timer = timer!("def_map.finalize.resolve_import_scopes");
            thread::sleep(Duration::from_millis(1));
        }

        let elapsed = run
            .finish()
            .duration("def_map.finalize.resolve_import_scopes")
            .expect("timer should record one duration");
        assert!(elapsed > Duration::ZERO);
    }
}
