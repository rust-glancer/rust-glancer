mod allocator;
mod memory;
mod package;
mod project;
mod stages;

use rg_project::{BuildProfile, Project};
use serde::Serialize;

pub(crate) use self::{
    allocator::{AllocatorPurgeReport, AllocatorReport, format_bytes},
    stages::{AnalysisSetupReport, BuildCheckpointReport, BuildProfileReport, format_duration_ms},
};

use self::{memory::MemoryReport, project::ProjectReport};

/// Machine-readable result of one `analyze` run.
#[derive(Debug, Serialize)]
pub(crate) struct AnalyzeReport {
    /// Root directory of the Cargo workspace that was analyzed.
    pub(crate) workspace_root: String,
    /// Coarse project counters describing the built analysis snapshot.
    pub(crate) project: ProjectReport,
    /// Setup timings collected before the analysis pipeline starts.
    pub(crate) analysis_setup: AnalysisSetupReport,
    /// Optional build-stage timings and memory samples from the analysis pipeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) build_profile: Option<BuildProfileReport>,
    /// Optional allocator statistics captured around the memory profile boundary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) allocator: Option<AllocatorReport>,
    /// Optional retained-memory breakdown for the final project snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memory: Option<MemoryReport>,
}

impl AnalyzeReport {
    pub(crate) fn build(
        project: &Project,
        analysis_setup: AnalysisSetupReport,
        build_profile: Option<&BuildProfile>,
        allocator: Option<AllocatorReport>,
        include_memory: bool,
    ) -> Self {
        Self {
            workspace_root: project.workspace().workspace_root().display().to_string(),
            project: ProjectReport::capture(project),
            analysis_setup,
            build_profile: build_profile.map(BuildProfileReport::capture),
            allocator,
            memory: include_memory.then(|| MemoryReport::capture(project)),
        }
    }

    pub(crate) fn render_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}
