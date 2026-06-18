use std::time::Duration;

use crate::{
    ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, ProfileMeasurement,
    ProfileMemorySnapshot, ProfileReport, ProfileTimer, ProfileUnit,
};

/// Typed handle for a counter metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterMetric {
    path: &'static str,
    scope: &'static str,
}

impl CounterMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self { path, scope }
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::counter(self.path, self.scope)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn inc(self) {
        self.add(1);
    }

    pub fn add(self, amount: u64) {
        crate::record_counter(self.path, amount);
    }
}

/// Typed handle for a gauge metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaugeMetric {
    path: &'static str,
    scope: &'static str,
    unit: ProfileUnit,
}

impl GaugeMetric {
    pub const fn new(path: &'static str, scope: &'static str, unit: ProfileUnit) -> Self {
        Self { path, scope, unit }
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::gauge(self.path, self.scope, self.unit)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn record(self, value: ProfileMeasurement) {
        crate::record_gauge(self.path, value);
    }

    pub fn record_empty(self) {
        self.record(ProfileMeasurement::Empty);
    }

    pub fn record_count(self, value: usize) {
        self.record(ProfileMeasurement::count(value));
    }

    pub fn record_integer(self, value: i64) {
        self.record(ProfileMeasurement::integer(value));
    }

    pub fn record_float(self, value: f64) {
        self.record(ProfileMeasurement::float(value));
    }

    pub fn record_bool(self, value: bool) {
        self.record(ProfileMeasurement::bool(value));
    }

    pub fn record_bytes(self, value: usize) {
        self.record(ProfileMeasurement::bytes(value));
    }

    pub fn record_optional_bytes(self, value: Option<usize>) {
        self.record(ProfileMeasurement::optional_bytes(value));
    }

    pub fn record_duration(self, value: Duration) {
        self.record(ProfileMeasurement::duration(value));
    }

    pub fn record_text(self, value: impl Into<String>) {
        self.record(ProfileMeasurement::text(value));
    }
}

/// Typed handle for an accumulated duration metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationMetric {
    path: &'static str,
    scope: &'static str,
}

impl DurationMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self { path, scope }
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::duration(self.path, self.scope)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn record(self, elapsed: Duration) {
        crate::record_duration(self.path, elapsed);
    }

    pub fn start_timer(self) -> ProfileTimer {
        crate::timer(self.path)
    }
}

/// Typed handle for a keyed counter metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedCounterMetric {
    path: &'static str,
    scope: &'static str,
    report: ProfileReport,
}

impl KeyedCounterMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self {
            path,
            scope,
            report: ProfileReport::new(),
        }
    }

    pub const fn report(mut self, report: ProfileReport) -> Self {
        self.report = report;
        self
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::keyed_counter(self.path, self.scope).report(self.report)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn inc(self, key: impl AsRef<str>) {
        self.add(key, 1);
    }

    pub fn add(self, key: impl AsRef<str>, amount: u64) {
        crate::record_keyed_counter(self.path, key, amount);
    }
}

/// Typed handle for a keyed duration metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedDurationMetric {
    path: &'static str,
    scope: &'static str,
    report: ProfileReport,
}

impl KeyedDurationMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self {
            path,
            scope,
            report: ProfileReport::new(),
        }
    }

    pub const fn report(mut self, report: ProfileReport) -> Self {
        self.report = report;
        self
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::keyed_duration(self.path, self.scope).report(self.report)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn record(self, key: impl AsRef<str>, elapsed: Duration) {
        crate::record_keyed_duration(self.path, key, elapsed);
    }
}

/// Typed handle for a checkpoint stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointMetric {
    path: &'static str,
    scope: &'static str,
    columns: &'static [ProfileCheckpointColumn],
}

impl CheckpointMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self {
            path,
            scope,
            columns: &[],
        }
    }

    pub const fn columns(mut self, columns: &'static [ProfileCheckpointColumn]) -> Self {
        self.columns = columns;
        self
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        ProfileDescriptor::checkpoint_stream(self.path, self.scope).checkpoint_columns(self.columns)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn record(self, label: impl Into<String>, values: Vec<ProfileCheckpointValue>) {
        crate::record_checkpoint(self.path, label, values);
    }
}

/// Typed handle for a detailed retained-memory snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshotMetric {
    path: &'static str,
    scope: &'static str,
    title: Option<&'static str>,
}

impl MemorySnapshotMetric {
    pub const fn new(path: &'static str, scope: &'static str) -> Self {
        Self {
            path,
            scope,
            title: None,
        }
    }

    pub const fn title(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    pub const fn descriptor(self) -> ProfileDescriptor {
        let descriptor = ProfileDescriptor::memory_snapshot(self.path, self.scope);
        match self.title {
            Some(title) => descriptor.title(title),
            None => descriptor,
        }
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub const fn title_text(self) -> Option<&'static str> {
        self.title
    }

    pub fn is_enabled(self) -> bool {
        crate::memory_snapshot_enabled(self.path)
    }

    pub fn record(self, snapshot: ProfileMemorySnapshot) {
        crate::record_memory_snapshot(self.path, snapshot);
    }
}
