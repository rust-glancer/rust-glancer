use std::time::Duration;

use rg_profile::{ProfileCheckpoint, ProfileMeasurement};
use serde::Serialize;

use super::allocator::AllocatorPurgeReport;
use crate::analyze::report::{ReportRowBuilder, ReportSectionBuilder, ReportTableBuilder};

/// Timings collected before the project pipeline itself starts.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AnalysisSetupReport {
    pub(crate) cargo_metadata_ms: f64,
    pub(crate) workspace_metadata_ms: f64,
    pub(crate) sysroot_discovery_ms: f64,
    pub(crate) total_ms: f64,
}

impl AnalysisSetupReport {
    pub(crate) fn new(
        cargo_metadata: Duration,
        workspace_metadata: Duration,
        sysroot_discovery: Duration,
    ) -> Self {
        Self {
            cargo_metadata_ms: duration_ms(cargo_metadata),
            workspace_metadata_ms: duration_ms(workspace_metadata),
            sysroot_discovery_ms: duration_ms(sysroot_discovery),
            total_ms: duration_ms(cargo_metadata + workspace_metadata + sysroot_discovery),
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.title("analysis setup profile");
        section.table("checkpoints", |table| {
            Self::append_timeline_columns(table);

            let mut elapsed_ms = 0.0;
            // The setup phase stores independent timings; the timeline table derives cumulative
            // elapsed time here so timeline-like renderers can use the same column schema.
            for (label, phase_elapsed_ms) in [
                ("cargo metadata", self.cargo_metadata_ms),
                ("workspace metadata", self.workspace_metadata_ms),
                ("sysroot discovery", self.sysroot_discovery_ms),
            ] {
                elapsed_ms += phase_elapsed_ms;
                table.row(|row| {
                    row.duration_ms("phase", phase_elapsed_ms)
                        .duration_ms("elapsed", elapsed_ms)
                        .text("checkpoint", label);
                });
            }
        });
    }

    fn append_timeline_columns(table: &mut ReportTableBuilder) {
        table
            .untitled()
            .duration_column("phase")
            .duration_column("elapsed")
            .text_column("checkpoint");
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildProfileReport {
    pub(crate) checkpoints: Vec<BuildCheckpointReport>,
}

impl BuildProfileReport {
    pub(crate) fn capture(checkpoints: &[ProfileCheckpoint]) -> Self {
        Self {
            checkpoints: checkpoints
                .iter()
                .map(BuildCheckpointReport::capture)
                .collect(),
        }
    }

    pub(super) fn append_document(
        &self,
        section: &mut ReportSectionBuilder,
        purge: Option<&AllocatorPurgeReport>,
    ) {
        let includes_memory = purge.is_some()
            || self
                .checkpoints
                .iter()
                .any(BuildCheckpointReport::has_memory);

        section.table("checkpoints", |table| {
            Self::append_checkpoint_columns(table, includes_memory);

            for checkpoint in &self.checkpoints {
                table.row(|row| checkpoint.append_row(row, includes_memory));
            }

            if let Some(purge) = purge {
                table.row(|row| self.append_allocator_purge_row(row, purge));
            }
        });
    }

    fn append_checkpoint_columns(table: &mut ReportTableBuilder, includes_memory: bool) {
        table
            .untitled()
            .duration_column("phase")
            .duration_column("elapsed");

        if includes_memory {
            table
                .bytes_column_as("retained_bytes", "rg_sampled")
                .bytes_column_as("active_retained_bytes", "rg_total")
                .bytes_column_as("allocated_bytes", "j_allocated")
                .bytes_column_as("active_bytes", "j_active")
                .bytes_column_as("resident_bytes", "j_resident")
                .bytes_column_as("mapped_bytes", "j_mapped");
        }

        table.text_column("checkpoint");
    }

    fn append_allocator_purge_row(&self, row: &mut ReportRowBuilder, purge: &AllocatorPurgeReport) {
        let project_checkpoint = self.checkpoints.last();
        row.empty("phase")
            .empty("elapsed")
            .optional_bytes(
                "retained_bytes",
                project_checkpoint.and_then(|checkpoint| checkpoint.retained_bytes),
            )
            .optional_bytes(
                "active_retained_bytes",
                project_checkpoint.and_then(|checkpoint| checkpoint.active_retained_bytes),
            )
            .optional_bytes(
                "allocated_bytes",
                purge.after.map(|stats| stats.allocated_bytes),
            )
            .optional_bytes("active_bytes", purge.after.map(|stats| stats.active_bytes))
            .optional_bytes(
                "resident_bytes",
                purge.after.map(|stats| stats.resident_bytes),
            )
            .optional_bytes("mapped_bytes", purge.after.map(|stats| stats.mapped_bytes))
            .text("checkpoint", "after allocator purge");
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildCheckpointReport {
    pub(crate) label: String,
    pub(crate) phase_elapsed_ms: f64,
    pub(crate) elapsed_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retained_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_retained_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) allocated_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resident_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mapped_bytes: Option<usize>,
}

impl BuildCheckpointReport {
    fn capture(checkpoint: &ProfileCheckpoint) -> Self {
        Self {
            label: checkpoint.label.to_string(),
            phase_elapsed_ms: duration_ms(checkpoint.phase_elapsed),
            elapsed_ms: duration_ms(checkpoint.elapsed),
            retained_bytes: Self::optional_bytes(checkpoint, "retained_bytes"),
            active_retained_bytes: Self::optional_bytes(checkpoint, "active_retained_bytes"),
            allocated_bytes: Self::optional_bytes(checkpoint, "allocated_bytes"),
            active_bytes: Self::optional_bytes(checkpoint, "active_bytes"),
            resident_bytes: Self::optional_bytes(checkpoint, "resident_bytes"),
            mapped_bytes: Self::optional_bytes(checkpoint, "mapped_bytes"),
        }
    }

    fn append_row(&self, row: &mut ReportRowBuilder, includes_memory: bool) {
        row.duration_ms("phase", self.phase_elapsed_ms)
            .duration_ms("elapsed", self.elapsed_ms)
            .text("checkpoint", &self.label);

        if includes_memory {
            row.optional_bytes("retained_bytes", self.retained_bytes)
                .optional_bytes("active_retained_bytes", self.active_retained_bytes)
                .optional_bytes("allocated_bytes", self.allocated_bytes)
                .optional_bytes("active_bytes", self.active_bytes)
                .optional_bytes("resident_bytes", self.resident_bytes)
                .optional_bytes("mapped_bytes", self.mapped_bytes);
        }
    }

    fn has_memory(&self) -> bool {
        self.retained_bytes.is_some()
            || self.active_retained_bytes.is_some()
            || self.allocated_bytes.is_some()
            || self.active_bytes.is_some()
            || self.resident_bytes.is_some()
            || self.mapped_bytes.is_some()
    }

    fn optional_bytes(checkpoint: &ProfileCheckpoint, key: &str) -> Option<usize> {
        let value = checkpoint.values.iter().find(|value| value.key == key)?;
        match &value.value {
            ProfileMeasurement::Empty => None,
            ProfileMeasurement::Bytes(value) => {
                Some(usize::try_from(*value).expect("profile byte values should fit in usize"))
            }
            value => {
                panic!("project build checkpoint value `{key}` should be bytes, got {value:?}")
            }
        }
    }
}

pub(super) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
