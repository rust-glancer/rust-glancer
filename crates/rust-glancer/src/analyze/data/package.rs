use rg_project::{Project, ProjectStats};
use serde::Serialize;

use crate::report::ReportFieldsBuilder;

#[derive(Debug, Serialize)]
pub(crate) struct PackageReport {
    pub(crate) total_count: usize,
    pub(crate) workspace_count: usize,
    pub(crate) residency_policy: String,
    pub(crate) resident_count: usize,
    pub(crate) offloaded_count: usize,
}

impl PackageReport {
    pub(crate) fn capture(project: &Project, stats: &ProjectStats) -> Self {
        let policy = project.package_residency_policy();
        let resident_count = stats.def_map.resident_package_count;
        let offloaded_count = stats.package_count - resident_count;

        Self {
            total_count: stats.package_count,
            workspace_count: stats.workspace_package_count,
            residency_policy: policy.label().to_string(),
            resident_count,
            offloaded_count,
        }
    }

    pub(super) fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("total_count", "total", self.total_count)
            .count_as("workspace_count", "workspace", self.workspace_count)
            .text("residency_policy", &self.residency_policy)
            .count_as("resident_count", "resident", self.resident_count)
            .count_as("offloaded_count", "offloaded", self.offloaded_count);
    }
}
