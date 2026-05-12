use rg_project::{PackageResidency, Project};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
pub(crate) struct PackageReport {
    pub(crate) total_count: usize,
    pub(crate) workspace_count: usize,
    pub(crate) residency_policy: String,
    pub(crate) resident_count: usize,
    pub(crate) offloaded_count: usize,
}

impl PackageReport {
    pub(crate) fn capture(project: &Project) -> Self {
        let stats = project.stats();
        let residency = project.package_residency_plan();
        let resident_count = residency
            .packages()
            .iter()
            .filter(|decision| matches!(decision, PackageResidency::Resident))
            .count();
        let offloaded_count = residency.packages().len() - resident_count;

        Self {
            total_count: stats.package_count,
            workspace_count: stats.workspace_package_count,
            residency_policy: residency.policy().config_name().to_string(),
            resident_count,
            offloaded_count,
        }
    }
}

impl fmt::Display for PackageReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({} workspace)",
            self.total_count, self.workspace_count
        )
    }
}
