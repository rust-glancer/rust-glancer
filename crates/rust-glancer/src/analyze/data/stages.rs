use std::time::Duration;

use serde::Serialize;

use crate::analyze::report::{ReportSectionBuilder, ReportTableBuilder};

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
        section.title("analysis setup");
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

pub(super) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
