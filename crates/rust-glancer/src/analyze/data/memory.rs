use rg_profile::{ProfileDescriptor, ProfileMemoryRecord, ProfileMemorySnapshot};
use rg_project::Project;
use rg_std::{MemoryRecord, MemoryRecorder, MemorySize};
use serde::Serialize;

use crate::analyze::report::{ReportSectionBuilder, ReportTableBuilder};

const TOP_MEMORY_ROWS: usize = 12;

#[derive(Debug, Serialize)]
pub(crate) struct MemoryReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) retained_bytes: usize,
    pub(crate) aggregate_bucket_count: usize,
    pub(crate) by_component: Vec<MemoryRow>,
    pub(crate) by_kind: Vec<MemoryRow>,
    pub(crate) top_paths: Vec<MemoryRow>,
    pub(crate) top_types: Vec<MemoryRow>,
}

impl MemoryReport {
    pub(crate) fn capture(project: &Project) -> Self {
        // Aggregate rows are intentionally shaped like the text report so humans can line up CI
        // comments with local `analyze --memory` output while scripts still consume raw bytes.
        let mut recorder = MemoryRecorder::new("project");
        project.record_memory_size(&mut recorder);
        let records = recorder
            .records()
            .iter()
            .map(MemoryReportRecord::from_project_record)
            .collect::<Vec<_>>();

        Self::from_records(None, None, recorder.total_bytes(), records)
    }

    pub(crate) fn capture_profile_snapshot(
        descriptor: ProfileDescriptor,
        snapshot: &ProfileMemorySnapshot,
    ) -> Self {
        Self::from_records(
            Some(descriptor.scope().to_string()),
            descriptor.title_text().map(ToString::to_string),
            snapshot.retained_bytes,
            snapshot
                .records
                .iter()
                .map(MemoryReportRecord::from_profile_record)
                .collect(),
        )
    }

    pub(crate) fn section_key(&self) -> String {
        let Some(point) = &self.point else {
            return "memory".to_string();
        };

        format!("memory_{}", profile_key(point))
    }

    fn from_records(
        point: Option<String>,
        label: Option<String>,
        retained_bytes: usize,
        records: Vec<MemoryReportRecord>,
    ) -> Self {
        Self {
            point,
            label,
            retained_bytes,
            aggregate_bucket_count: records.len(),
            by_component: memory_rows(top_level_totals(&records), usize::MAX),
            by_kind: memory_rows(kind_totals(&records), usize::MAX),
            top_paths: memory_rows(
                string_totals(
                    records
                        .iter()
                        .map(|record| (record.path.as_str(), record.bytes)),
                ),
                TOP_MEMORY_ROWS,
            ),
            top_types: memory_rows(
                string_totals(
                    records
                        .iter()
                        .map(|record| (record.type_name.as_str(), record.bytes)),
                ),
                TOP_MEMORY_ROWS,
            ),
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        if let Some(label) = &self.label {
            section.title(format!("memory: {label}"));
        }

        section.fields("summary", |fields| {
            if let Some(point) = &self.point {
                fields.text("point", point);
            }
            if let Some(label) = &self.label {
                fields.text("label", label);
            }
            fields
                .bytes_as("retained_bytes", "retained", self.retained_bytes)
                .count_as(
                    "aggregate_bucket_count",
                    "aggregate buckets",
                    self.aggregate_bucket_count,
                );
        });

        section.table("by_component", |table| {
            MemoryRow::append_table(table, &self.by_component);
        });
        section.table("by_kind", |table| {
            MemoryRow::append_table(table, &self.by_kind);
        });
        section.table("top_paths", |table| {
            MemoryRow::append_table(table, &self.top_paths);
        });
        section.table("top_types", |table| {
            MemoryRow::append_table(table, &self.top_types);
        });
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryRow {
    pub(crate) label: String,
    pub(crate) bytes: usize,
}

impl MemoryRow {
    fn append_table(table: &mut ReportTableBuilder, rows: &[Self]) {
        table.bytes_column("bytes").text_column("label");

        for row in rows {
            table.row(|table_row| {
                table_row
                    .bytes("bytes", row.bytes)
                    .text("label", &row.label);
            });
        }
    }
}

struct MemoryReportRecord {
    path: String,
    type_name: String,
    kind: String,
    bytes: usize,
}

impl MemoryReportRecord {
    fn from_project_record(record: &MemoryRecord) -> Self {
        Self {
            path: record.path.clone(),
            type_name: record.type_name.clone(),
            kind: record.kind.as_str().to_string(),
            bytes: record.bytes,
        }
    }

    fn from_profile_record(record: &ProfileMemoryRecord) -> Self {
        Self {
            path: record.path.clone(),
            type_name: record.type_name.clone(),
            kind: record.kind.clone(),
            bytes: record.bytes,
        }
    }
}

fn profile_key(path: &str) -> String {
    path.replace('.', "_")
}

fn memory_rows(rows: Vec<(String, usize)>, limit: usize) -> Vec<MemoryRow> {
    rows.into_iter()
        .take(limit)
        .map(|(label, bytes)| MemoryRow { label, bytes })
        .collect()
}

fn top_level_totals(records: &[MemoryReportRecord]) -> Vec<(String, usize)> {
    string_totals(records.iter().map(|record| {
        let path = top_level_path(&record.path);
        (path, record.bytes)
    }))
}

fn top_level_path(path: &str) -> String {
    let mut parts = path.split('.');
    let Some(root) = parts.next() else {
        return path.to_string();
    };
    let Some(child) = parts.next() else {
        return root.to_string();
    };

    format!("{root}.{child}")
}

fn kind_totals(records: &[MemoryReportRecord]) -> Vec<(String, usize)> {
    string_totals(
        records
            .iter()
            .map(|record| (record.kind.as_str(), record.bytes)),
    )
}

fn string_totals<S>(items: impl IntoIterator<Item = (S, usize)>) -> Vec<(String, usize)>
where
    S: Into<String>,
{
    let mut totals = std::collections::BTreeMap::<String, usize>::new();
    for (label, bytes) in items {
        *totals.entry(label.into()).or_default() += bytes;
    }

    let mut rows = totals.into_iter().collect::<Vec<_>>();
    rows.sort_by(|(left_label, left_bytes), (right_label, right_bytes)| {
        right_bytes
            .cmp(left_bytes)
            .then_with(|| left_label.cmp(right_label))
    });
    rows
}
