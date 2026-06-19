use std::collections::{BTreeMap, BTreeSet};

use rg_profile::{
    ProfileCheckpoint, ProfileCheckpointColumn, ProfileCheckpointValue, ProfileEntry,
    ProfileInstrumentKind, ProfileKeyedCounter, ProfileKeyedDuration, ProfileMeasurement,
    ProfileReportSort, ProfileSnapshot, ProfileUnit, ProfileValue,
};
use serde::Serialize;

use super::stages::duration_ms;
use crate::analyze::report::{
    ReportAlign, ReportFieldsBuilder, ReportRowBuilder, ReportSectionBuilder, ReportUnit,
    ReportValue,
};

/// Serializable view of the dynamic profile snapshot produced by `rg_profile`.
#[derive(Debug, Serialize)]
pub(crate) struct ProfileSnapshotReport {
    pub(crate) entries: Vec<ProfileEntryReport>,
}

impl ProfileSnapshotReport {
    pub(crate) fn capture(snapshot: &ProfileSnapshot) -> Self {
        Self {
            entries: snapshot
                .entries()
                .iter()
                .map(ProfileEntryReport::capture)
                .collect(),
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.title("profile snapshot");

        // Scalars stay grouped by activation scope so broad profile sections read as compact field
        // blocks instead of a single long list of fully qualified paths.
        let mut scalar_entries = BTreeMap::<&str, Vec<&ProfileEntryReport>>::new();
        for entry in &self.entries {
            if entry.value.as_report_value().is_some() {
                scalar_entries
                    .entry(entry.scope.as_str())
                    .or_default()
                    .push(entry);
            }
        }

        for (scope, entries) in scalar_entries {
            section.fields(profile_key(scope), |fields| {
                fields.title(profile_title(scope));
                for entry in entries {
                    entry.append_scalar_field(fields);
                }
            });
        }

        for entry in &self.entries {
            entry.append_table(section);
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileEntryReport {
    pub(crate) path: String,
    pub(crate) scope: String,
    pub(crate) kind: &'static str,
    pub(crate) unit: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) limit: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) checkpoint_columns: Vec<ProfileCheckpointColumnReport>,
    pub(crate) value: ProfileValueReport,
}

impl ProfileEntryReport {
    fn capture(entry: &ProfileEntry) -> Self {
        let descriptor = entry.descriptor();
        Self {
            path: descriptor.path().to_string(),
            scope: descriptor.scope().to_string(),
            kind: instrument_kind(descriptor.kind()),
            unit: unit(descriptor.unit()),
            title: descriptor.title_text().map(ToString::to_string),
            sort: descriptor.report_hints().sort.map(report_sort),
            limit: descriptor.report_hints().limit,
            checkpoint_columns: descriptor
                .checkpoint_columns_slice()
                .iter()
                .map(ProfileCheckpointColumnReport::capture)
                .collect(),
            value: ProfileValueReport::capture(entry.value()),
        }
    }

    fn append_scalar_field(&self, fields: &mut ReportFieldsBuilder) {
        let Some(value) = self.value.as_report_value() else {
            return;
        };

        fields.value_as(profile_key(&self.path), self.entry_title(), value);
    }

    fn append_table(&self, section: &mut ReportSectionBuilder) {
        match &self.value {
            ProfileValueReport::KeyedCounters(counters) => {
                section.table(profile_key(&self.path), |table| {
                    table
                        .title(self.entry_title())
                        .count_column("count")
                        .text_column("key");

                    let mut rows = counters.iter().collect::<Vec<_>>();
                    if self.sort == Some("count_desc") {
                        rows.sort_by(|left, right| {
                            right
                                .count
                                .cmp(&left.count)
                                .then_with(|| left.key.cmp(&right.key))
                        });
                    }
                    rows.truncate(self.limit.unwrap_or(rows.len()));

                    for counter in rows {
                        table.row(|row| {
                            row.value("count", ReportValue::Count(counter.count))
                                .text("key", &counter.key);
                        });
                    }
                });
            }
            ProfileValueReport::KeyedDurations(durations) => {
                section.table(profile_key(&self.path), |table| {
                    table
                        .title(self.entry_title())
                        .duration_column_as("total_ms", "total")
                        .duration_column_as("average_ms", "avg")
                        .duration_column_as("max_ms", "max")
                        .count_column("count")
                        .text_column("key");

                    let mut rows = durations.iter().collect::<Vec<_>>();
                    if self.sort == Some("total_duration_desc") {
                        rows.sort_by(|left, right| {
                            right
                                .total_ms
                                .total_cmp(&left.total_ms)
                                .then_with(|| left.key.cmp(&right.key))
                        });
                    }
                    rows.truncate(self.limit.unwrap_or(rows.len()));

                    for duration in rows {
                        table.row(|row| {
                            row.duration_ms("total_ms", duration.total_ms)
                                .duration_ms("average_ms", duration.average_ms)
                                .duration_ms("max_ms", duration.max_ms)
                                .value("count", ReportValue::Count(duration.count))
                                .text("key", &duration.key);
                        });
                    }
                });
            }
            ProfileValueReport::Checkpoints(checkpoints) => {
                self.append_checkpoint_table(section, checkpoints);
            }
            ProfileValueReport::Counter(_)
            | ProfileValueReport::Gauge(_)
            | ProfileValueReport::DurationMs(_)
            | ProfileValueReport::MemorySnapshot(_) => {}
        }
    }

    fn append_checkpoint_table(
        &self,
        section: &mut ReportSectionBuilder,
        checkpoints: &[ProfileCheckpointReport],
    ) {
        section.table(profile_key(&self.path), |table| {
            table
                .title(self.entry_title())
                .duration_column("phase")
                .duration_column("elapsed");

            let value_columns = checkpoint_value_columns(&self.checkpoint_columns, checkpoints);
            for column in &value_columns {
                table.column_as(
                    column.key.clone(),
                    column.title.clone(),
                    ReportAlign::Right,
                    column.unit,
                );
            }
            table.text_column("checkpoint");

            for checkpoint in checkpoints {
                table.row(|row| {
                    row.duration_ms("phase", checkpoint.phase_elapsed_ms)
                        .duration_ms("elapsed", checkpoint.elapsed_ms);
                    checkpoint.append_values(row);
                    row.text("checkpoint", &checkpoint.label);
                });
            }
        });
    }

    fn entry_title(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }

        let suffix = self
            .path
            .strip_prefix(&self.scope)
            .and_then(|suffix| suffix.strip_prefix('.'))
            .unwrap_or(&self.path);
        profile_title(suffix)
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProfileCheckpointColumnReport {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) unit: Option<ReportUnit>,
}

impl ProfileCheckpointColumnReport {
    fn capture(column: &ProfileCheckpointColumn) -> Self {
        Self {
            key: column.key.to_string(),
            title: column.title.to_string(),
            unit: report_unit(column.unit),
        }
    }

    fn inferred(key: impl Into<String>, unit: Option<ReportUnit>) -> Self {
        let key = key.into();
        Self {
            title: profile_title(&key),
            key,
            unit,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ProfileValueReport {
    Counter(u64),
    Gauge(ProfileMeasurementReport),
    DurationMs(f64),
    KeyedCounters(Vec<ProfileKeyedCounterReport>),
    KeyedDurations(Vec<ProfileKeyedDurationReport>),
    Checkpoints(Vec<ProfileCheckpointReport>),
    MemorySnapshot(ProfileMemorySnapshotReport),
}

impl ProfileValueReport {
    fn capture(value: &ProfileValue) -> Self {
        match value {
            ProfileValue::Counter(value) => Self::Counter(*value),
            ProfileValue::Gauge(value) => Self::Gauge(ProfileMeasurementReport::capture(value)),
            ProfileValue::Duration(value) => Self::DurationMs(duration_ms(*value)),
            ProfileValue::KeyedCounters(counters) => Self::KeyedCounters(
                counters
                    .iter()
                    .map(ProfileKeyedCounterReport::capture)
                    .collect(),
            ),
            ProfileValue::KeyedDurations(durations) => Self::KeyedDurations(
                durations
                    .iter()
                    .map(ProfileKeyedDurationReport::capture)
                    .collect(),
            ),
            ProfileValue::Checkpoints(checkpoints) => Self::Checkpoints(
                checkpoints
                    .iter()
                    .map(ProfileCheckpointReport::capture)
                    .collect(),
            ),
            ProfileValue::MemorySnapshot(snapshot) => {
                Self::MemorySnapshot(ProfileMemorySnapshotReport::capture(snapshot))
            }
        }
    }

    fn as_report_value(&self) -> Option<ReportValue> {
        match self {
            Self::Counter(value) => Some(ReportValue::Count(*value)),
            Self::Gauge(value) => Some(value.as_report_value()),
            Self::DurationMs(value) => Some(ReportValue::DurationMs(*value)),
            Self::KeyedCounters(_)
            | Self::KeyedDurations(_)
            | Self::Checkpoints(_)
            | Self::MemorySnapshot(_) => None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileMemorySnapshotReport {
    pub(crate) retained_bytes: usize,
    pub(crate) aggregate_bucket_count: usize,
}

impl ProfileMemorySnapshotReport {
    fn capture(snapshot: &rg_profile::ProfileMemorySnapshot) -> Self {
        Self {
            retained_bytes: snapshot.retained_bytes,
            aggregate_bucket_count: snapshot.records.len(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileKeyedCounterReport {
    pub(crate) key: String,
    pub(crate) count: u64,
}

impl ProfileKeyedCounterReport {
    fn capture(counter: &ProfileKeyedCounter) -> Self {
        Self {
            key: counter.key.clone(),
            count: counter.count,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileKeyedDurationReport {
    pub(crate) key: String,
    pub(crate) count: u64,
    pub(crate) total_ms: f64,
    pub(crate) average_ms: f64,
    pub(crate) max_ms: f64,
}

impl ProfileKeyedDurationReport {
    fn capture(duration: &ProfileKeyedDuration) -> Self {
        Self {
            key: duration.key.clone(),
            count: duration.count,
            total_ms: duration_ms(duration.total),
            average_ms: duration_ms(duration.average()),
            max_ms: duration_ms(duration.max),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileCheckpointReport {
    pub(crate) label: String,
    pub(crate) phase_elapsed_ms: f64,
    pub(crate) elapsed_ms: f64,
    pub(crate) values: Vec<ProfileCheckpointValueReport>,
}

impl ProfileCheckpointReport {
    fn capture(checkpoint: &ProfileCheckpoint) -> Self {
        Self {
            label: checkpoint.label.clone(),
            phase_elapsed_ms: duration_ms(checkpoint.phase_elapsed),
            elapsed_ms: duration_ms(checkpoint.elapsed),
            values: checkpoint
                .values
                .iter()
                .map(ProfileCheckpointValueReport::capture)
                .collect(),
        }
    }
}

impl ProfileCheckpointReport {
    fn append_values(&self, row: &mut ReportRowBuilder) {
        for value in &self.values {
            row.value(value.key.clone(), value.value.as_report_value());
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileCheckpointValueReport {
    pub(crate) key: String,
    pub(crate) value: ProfileMeasurementReport,
}

impl ProfileCheckpointValueReport {
    fn capture(value: &ProfileCheckpointValue) -> Self {
        Self {
            key: value.key.clone(),
            value: ProfileMeasurementReport::capture(&value.value),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ProfileMeasurementReport {
    Empty,
    Count(u64),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Bytes(u64),
    DurationMs(f64),
    Text(String),
}

impl ProfileMeasurementReport {
    fn capture(measurement: &ProfileMeasurement) -> Self {
        match measurement {
            ProfileMeasurement::Empty => Self::Empty,
            ProfileMeasurement::Count(value) => Self::Count(*value),
            ProfileMeasurement::Integer(value) => Self::Integer(*value),
            ProfileMeasurement::Float(value) => Self::Float(*value),
            ProfileMeasurement::Bool(value) => Self::Bool(*value),
            ProfileMeasurement::Bytes(value) => Self::Bytes(*value),
            ProfileMeasurement::Duration(value) => Self::DurationMs(duration_ms(*value)),
            ProfileMeasurement::Text(value) => Self::Text(value.clone()),
        }
    }

    fn as_report_value(&self) -> ReportValue {
        match self {
            Self::Empty => ReportValue::Empty,
            Self::Count(value) => ReportValue::Count(*value),
            Self::Integer(value) => ReportValue::Integer(*value),
            Self::Float(value) => ReportValue::Float(*value),
            Self::Bool(value) => ReportValue::Bool(*value),
            Self::Bytes(value) => ReportValue::Bytes(*value),
            Self::DurationMs(value) => ReportValue::DurationMs(*value),
            Self::Text(value) => ReportValue::Text(value.clone()),
        }
    }

    fn report_unit(&self) -> Option<ReportUnit> {
        match self {
            Self::Empty | Self::Integer(_) | Self::Float(_) | Self::Bool(_) | Self::Text(_) => None,
            Self::Count(_) => Some(ReportUnit::Count),
            Self::Bytes(_) => Some(ReportUnit::Bytes),
            Self::DurationMs(_) => Some(ReportUnit::Duration),
        }
    }
}

fn instrument_kind(kind: ProfileInstrumentKind) -> &'static str {
    match kind {
        ProfileInstrumentKind::Counter => "counter",
        ProfileInstrumentKind::Gauge => "gauge",
        ProfileInstrumentKind::Duration => "duration",
        ProfileInstrumentKind::KeyedCounter => "keyed_counter",
        ProfileInstrumentKind::KeyedDuration => "keyed_duration",
        ProfileInstrumentKind::CheckpointStream => "checkpoint_stream",
        ProfileInstrumentKind::MemorySnapshot => "memory_snapshot",
    }
}

fn unit(unit: ProfileUnit) -> &'static str {
    match unit {
        ProfileUnit::None => "none",
        ProfileUnit::Count => "count",
        ProfileUnit::Bytes => "bytes",
        ProfileUnit::Duration => "duration",
        ProfileUnit::Percent => "percent",
    }
}

fn report_unit(unit: ProfileUnit) -> Option<ReportUnit> {
    match unit {
        ProfileUnit::None => None,
        ProfileUnit::Count => Some(ReportUnit::Count),
        ProfileUnit::Bytes => Some(ReportUnit::Bytes),
        ProfileUnit::Duration => Some(ReportUnit::Duration),
        ProfileUnit::Percent => Some(ReportUnit::Percent),
    }
}

fn report_sort(sort: ProfileReportSort) -> &'static str {
    match sort {
        ProfileReportSort::KeyAscending => "key_asc",
        ProfileReportSort::CountDescending => "count_desc",
        ProfileReportSort::TotalDurationDescending => "total_duration_desc",
    }
}

fn profile_key(path: &str) -> String {
    path.replace('.', "_")
}

fn profile_title(path: &str) -> String {
    path.replace(['.', '_'], " ")
}

fn checkpoint_value_columns(
    declared_columns: &[ProfileCheckpointColumnReport],
    checkpoints: &[ProfileCheckpointReport],
) -> Vec<ProfileCheckpointColumnReport> {
    let mut columns = declared_columns.to_vec();
    let declared_keys = declared_columns
        .iter()
        .map(|column| column.key.as_str())
        .collect::<BTreeSet<_>>();
    let mut inferred_columns = BTreeMap::<String, ProfileCheckpointColumnReport>::new();

    for checkpoint in checkpoints {
        for value in &checkpoint.values {
            if declared_keys.contains(value.key.as_str()) {
                continue;
            }

            inferred_columns
                .entry(value.key.clone())
                .or_insert_with(|| {
                    ProfileCheckpointColumnReport::inferred(
                        value.key.clone(),
                        value.value.report_unit(),
                    )
                });
        }
    }

    columns.extend(inferred_columns.into_values());
    columns
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rg_profile::{
        ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, ProfileMeasurement,
        test_support::ProfileTest,
    };

    use crate::analyze::report::{ReportBlock, ReportUnit};

    use super::*;

    #[test]
    fn captures_dynamic_profile_snapshot_for_json_report() {
        let descriptors = [
            ProfileDescriptor::counter("test.scope.counter", "test.scope"),
            ProfileDescriptor::keyed_duration("test.scope.detail.by_key", "test.scope.detail"),
        ];
        let run = ProfileTest::start(&descriptors, "test.scope.detail");

        rg_profile::record_counter("test.scope.counter", 2);
        rg_profile::record_keyed_duration(
            "test.scope.detail.by_key",
            "item",
            Duration::from_millis(5),
        );
        let snapshot = run.finish().into_inner();

        let report = ProfileSnapshotReport::capture(&snapshot);

        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.path == "test.scope.counter"),
            "summary entries should be retained in the JSON-facing report"
        );
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.path == "test.scope.detail.by_key"),
            "keyed entries should be retained in the JSON-facing report"
        );

        let document = crate::analyze::report::ReportDocument::builder("profile_test")
            .section("profile", |section| report.append_document(section))
            .build();
        let section = document
            .sections
            .first()
            .expect("profile report should render a section");
        assert!(
            section
                .blocks
                .iter()
                .any(|block| matches!(block, crate::analyze::report::ReportBlock::Fields { .. })),
            "scalar profile entries should render as report fields"
        );
        assert!(
            section
                .blocks
                .iter()
                .any(|block| matches!(block, crate::analyze::report::ReportBlock::Table { .. })),
            "keyed profile entries should render as report tables"
        );
    }

    #[test]
    fn renders_declared_checkpoint_columns() {
        static CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[
            ProfileCheckpointColumn::bytes("retained_bytes", "rg_sampled"),
            ProfileCheckpointColumn::count("packages", "packages"),
        ];
        let descriptors =
            [
                ProfileDescriptor::checkpoint_stream("test.scope.checkpoints", "test.scope")
                    .checkpoint_columns(CHECKPOINT_COLUMNS),
            ];
        let run = ProfileTest::start(&descriptors, "test.scope");

        rg_profile::record_checkpoint(
            "test.scope.checkpoints",
            "after parse",
            vec![
                ProfileCheckpointValue::new("retained_bytes", ProfileMeasurement::bytes(64)),
                ProfileCheckpointValue::new("packages", ProfileMeasurement::count(3)),
                ProfileCheckpointValue::new("allocated_bytes", ProfileMeasurement::bytes(128)),
            ],
        );
        let snapshot = run.finish().into_inner();

        let report = ProfileSnapshotReport::capture(&snapshot);
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.path == "test.scope.checkpoints")
            .expect("checkpoint entry should be retained in the JSON-facing report");
        assert_eq!(
            entry
                .checkpoint_columns
                .iter()
                .map(|column| column.key.as_str())
                .collect::<Vec<_>>(),
            ["retained_bytes", "packages"],
            "declared checkpoint column order should survive report capture",
        );

        let retained_column = entry
            .checkpoint_columns
            .iter()
            .find(|column| column.key == "retained_bytes")
            .expect("declared retained-memory column should be captured");
        assert_eq!(
            retained_column.title, "rg_sampled",
            "declared checkpoint titles should survive report capture",
        );
        assert!(
            matches!(retained_column.unit, Some(ReportUnit::Bytes)),
            "declared checkpoint units should survive report capture",
        );

        let document = crate::analyze::report::ReportDocument::builder("profile_test")
            .section("profile", |section| report.append_document(section))
            .build();
        let table_columns = document
            .sections
            .first()
            .expect("profile report should render a section")
            .blocks
            .iter()
            .find_map(|block| match block {
                ReportBlock::Table { key, columns, .. } if key == "test_scope_checkpoints" => {
                    Some(columns)
                }
                _ => None,
            })
            .expect("checkpoint entries should render as report tables");
        assert_eq!(
            table_columns
                .iter()
                .map(|column| column.key.as_str())
                .collect::<Vec<_>>(),
            [
                "phase",
                "elapsed",
                "retained_bytes",
                "packages",
                "allocated_bytes",
                "checkpoint",
            ],
            "checkpoint tables should render declared columns first, then inferred columns",
        );

        let retained_column = table_columns
            .iter()
            .find(|column| column.key == "retained_bytes")
            .expect("checkpoint table should use the retained-memory column");
        assert_eq!(
            retained_column.title, "rg_sampled",
            "checkpoint table should use the declared column title",
        );
        assert!(
            matches!(retained_column.unit, Some(ReportUnit::Bytes)),
            "checkpoint table should use the declared column unit",
        );
    }
}
