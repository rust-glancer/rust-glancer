use std::fmt;

use rg_memsize::{MemoryRecord, MemoryRecorder, MemorySize};
use rg_project::{BuildStageMemorySnapshot, Project};
use serde::Serialize;

use super::allocator::format_bytes;

const TOP_MEMORY_ROWS: usize = 12;

#[derive(Debug, Serialize)]
pub(crate) struct MemoryReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stage: Option<String>,
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
        let records = recorder.records();

        Self::from_records(None, recorder.total_bytes(), &records)
    }

    pub(crate) fn capture_stage(snapshot: &BuildStageMemorySnapshot) -> Self {
        Self::from_records(
            Some(snapshot.label().to_string()),
            snapshot.retained_bytes(),
            snapshot.records(),
        )
    }

    fn from_records(
        stage: Option<String>,
        retained_bytes: usize,
        records: &[MemoryRecord],
    ) -> Self {
        Self {
            stage,
            retained_bytes,
            aggregate_bucket_count: records.len(),
            by_component: memory_rows(top_level_totals(records), usize::MAX),
            by_kind: memory_rows(kind_totals(records), usize::MAX),
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
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryRow {
    pub(crate) label: String,
    pub(crate) bytes: usize,
}

impl fmt::Display for MemoryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(stage) = &self.stage {
            writeln!(f, "memory stage: {stage}")?;
        }
        writeln!(
            f,
            "memory: {} retained across {} aggregate buckets",
            format_bytes(self.retained_bytes),
            self.aggregate_bucket_count,
        )?;

        render_memory_section(f, "memory by component", &self.by_component)?;
        render_memory_section(f, "memory by kind", &self.by_kind)?;
        render_memory_section(f, "top memory paths", &self.top_paths)?;
        render_memory_section(f, "top memory types", &self.top_types)
    }
}

impl fmt::Display for MemoryRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:>10}  {}", format_bytes(self.bytes), self.label)
    }
}

fn render_memory_section(
    f: &mut fmt::Formatter<'_>,
    title: &str,
    rows: &[MemoryRow],
) -> fmt::Result {
    writeln!(f, "{title}:")?;

    for row in rows {
        writeln!(f, "  {row}")?;
    }

    Ok(())
}

fn memory_rows(rows: Vec<(String, usize)>, limit: usize) -> Vec<MemoryRow> {
    rows.into_iter()
        .take(limit)
        .map(|(label, bytes)| MemoryRow { label, bytes })
        .collect()
}

fn top_level_totals(records: &[MemoryRecord]) -> Vec<(String, usize)> {
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

fn kind_totals(records: &[MemoryRecord]) -> Vec<(String, usize)> {
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
