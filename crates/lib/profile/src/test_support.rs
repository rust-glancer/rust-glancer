use std::sync::{Mutex, MutexGuard};

use crate::{
    CounterMetric, GaugeMetric, KeyedCounterMetric, KeyedDurationMetric, ProfileDescriptor,
    ProfileFilter, ProfileMeasurement, ProfileRegistry, ProfileRun, ProfileSnapshot,
};

static PROFILE_TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn test_registry(descriptors: &[ProfileDescriptor]) -> ProfileRegistry {
    ProfileRegistry::new(descriptors.iter().copied())
        .expect("profile test descriptors should be valid")
}

/// Test guard for one scoped profiling run.
pub struct ProfileTest {
    run: ProfileRun,
    _lock: MutexGuard<'static, ()>,
}

impl ProfileTest {
    pub fn start(descriptors: &[ProfileDescriptor], filter: &str) -> Self {
        let lock = PROFILE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let registry = test_registry(descriptors);
        let filter = ProfileFilter::parse(filter)
            .unwrap_or_else(|error| panic!("profile test filter `{filter}` should parse: {error}"));
        let run = ProfileRun::start_with_registry(registry, filter)
            .expect("profile test run should start");

        Self { run, _lock: lock }
    }

    pub fn finish(self) -> TestSnapshot {
        TestSnapshot {
            snapshot: self.run.finish(),
        }
    }
}

pub struct TestSnapshot {
    snapshot: ProfileSnapshot,
}

impl TestSnapshot {
    pub fn inner(&self) -> &ProfileSnapshot {
        &self.snapshot
    }

    pub fn into_inner(self) -> ProfileSnapshot {
        self.snapshot
    }

    pub fn assert_counter(&self, metric: CounterMetric, expected: u64) {
        self.assert_counter_path_impl(metric.path(), expected, None);
    }

    pub fn assert_counter_with_message(&self, metric: CounterMetric, expected: u64, message: &str) {
        self.assert_counter_path_impl(metric.path(), expected, Some(message));
    }

    pub fn assert_counter_path(&self, path: &str, expected: u64) {
        self.assert_counter_path_impl(path, expected, None);
    }

    pub fn assert_counter_path_with_message(&self, path: &str, expected: u64, message: &str) {
        self.assert_counter_path_impl(path, expected, Some(message));
    }

    pub fn assert_counter_satisfies(
        &self,
        metric: CounterMetric,
        predicate: impl FnOnce(u64) -> bool,
    ) {
        self.assert_counter_path_satisfies_impl(metric.path(), predicate, None);
    }

    pub fn assert_counter_satisfies_with_message(
        &self,
        metric: CounterMetric,
        predicate: impl FnOnce(u64) -> bool,
        message: &str,
    ) {
        self.assert_counter_path_satisfies_impl(metric.path(), predicate, Some(message));
    }

    pub fn assert_counter_path_satisfies(&self, path: &str, predicate: impl FnOnce(u64) -> bool) {
        self.assert_counter_path_satisfies_impl(path, predicate, None);
    }

    pub fn assert_counter_path_satisfies_with_message(
        &self,
        path: &str,
        predicate: impl FnOnce(u64) -> bool,
        message: &str,
    ) {
        self.assert_counter_path_satisfies_impl(path, predicate, Some(message));
    }

    pub fn assert_keyed_counter(&self, metric: KeyedCounterMetric, key: &str, expected: u64) {
        self.assert_keyed_counter_impl(metric.path(), key, expected, None);
    }

    pub fn assert_keyed_counter_with_message(
        &self,
        metric: KeyedCounterMetric,
        key: &str,
        expected: u64,
        message: &str,
    ) {
        self.assert_keyed_counter_impl(metric.path(), key, expected, Some(message));
    }

    pub fn assert_keyed_duration_count(
        &self,
        metric: KeyedDurationMetric,
        key: &str,
        expected: u64,
    ) {
        self.assert_keyed_duration_count_impl(metric.path(), key, expected, None);
    }

    pub fn assert_keyed_duration_count_with_message(
        &self,
        metric: KeyedDurationMetric,
        key: &str,
        expected: u64,
        message: &str,
    ) {
        self.assert_keyed_duration_count_impl(metric.path(), key, expected, Some(message));
    }

    pub fn assert_gauge(&self, metric: GaugeMetric, expected: ProfileMeasurement) {
        self.assert_gauge_impl(metric.path(), expected, None);
    }

    pub fn assert_gauge_with_message(
        &self,
        metric: GaugeMetric,
        expected: ProfileMeasurement,
        message: &str,
    ) {
        self.assert_gauge_impl(metric.path(), expected, Some(message));
    }

    pub fn assert_gauge_count(&self, metric: GaugeMetric, expected: usize) {
        self.assert_gauge(metric, ProfileMeasurement::count(expected));
    }

    pub fn assert_gauge_count_with_message(
        &self,
        metric: GaugeMetric,
        expected: usize,
        message: &str,
    ) {
        self.assert_gauge_with_message(metric, ProfileMeasurement::count(expected), message);
    }

    pub fn assert_gauge_bool(&self, metric: GaugeMetric, expected: bool) {
        self.assert_gauge(metric, ProfileMeasurement::bool(expected));
    }

    pub fn assert_gauge_bool_with_message(
        &self,
        metric: GaugeMetric,
        expected: bool,
        message: &str,
    ) {
        self.assert_gauge_with_message(metric, ProfileMeasurement::bool(expected), message);
    }

    fn assert_counter_path_impl(&self, path: &str, expected: u64, message: Option<&str>) {
        let actual = self.snapshot.counter(path);
        let details = format!("expected counter `{path}` to be {expected}, got {actual:?}");
        assert_eq!(
            actual,
            Some(expected),
            "{}",
            Self::assertion_message(details, message)
        );
    }

    fn assert_counter_path_satisfies_impl(
        &self,
        path: &str,
        predicate: impl FnOnce(u64) -> bool,
        message: Option<&str>,
    ) {
        let Some(actual) = self.snapshot.counter(path) else {
            let details = format!("expected counter `{path}` to be present");
            panic!("{}", Self::assertion_message(details, message));
        };
        let details = format!("counter `{path}` had unexpected value {actual}");
        assert!(
            predicate(actual),
            "{}",
            Self::assertion_message(details, message)
        );
    }

    fn assert_keyed_counter_impl(
        &self,
        path: &str,
        key: &str,
        expected: u64,
        message: Option<&str>,
    ) {
        let actual = self.snapshot.keyed_counter(path, key);
        let details = format!(
            "expected keyed counter `{path}` for key `{key}` to be {expected}, got {actual:?}"
        );
        assert_eq!(
            actual,
            Some(expected),
            "{}",
            Self::assertion_message(details, message)
        );
    }

    fn assert_keyed_duration_count_impl(
        &self,
        path: &str,
        key: &str,
        expected: u64,
        message: Option<&str>,
    ) {
        let actual = self
            .snapshot
            .keyed_duration(path, key)
            .map(|duration| duration.count);
        let details = format!(
            "expected keyed duration `{path}` for key `{key}` to have count {expected}, got {actual:?}"
        );
        assert_eq!(
            actual,
            Some(expected),
            "{}",
            Self::assertion_message(details, message)
        );
    }

    fn assert_gauge_impl(&self, path: &str, expected: ProfileMeasurement, message: Option<&str>) {
        let actual = self.snapshot.gauge(path).cloned();
        let details = format!("expected gauge `{path}` to be {expected:?}, got {actual:?}");
        assert_eq!(
            actual,
            Some(expected),
            "{}",
            Self::assertion_message(details, message)
        );
    }

    fn assertion_message(details: String, message: Option<&str>) -> String {
        match message {
            Some(message) => format!("{message}: {details}"),
            None => details,
        }
    }
}
