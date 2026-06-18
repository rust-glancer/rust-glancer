mod allocator;
mod memory;
mod package;
mod profile;
mod project;
mod stages;

use rg_profile::ProfileSnapshot;
use rg_project::Project;
use serde::Serialize;

pub(crate) use self::{
    allocator::{AllocatorPurgeReport, AllocatorReport},
    stages::AnalysisSetupReport,
};

use self::{
    memory::MemoryReport, profile::ProfileSnapshotReport, project::ProjectReport,
    stages::BuildProfileReport,
};
use super::report::ReportDocument;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReportDocumentOptions {
    pub(crate) include_memory: bool,
}

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
    /// Optional dynamic profile snapshot captured through the implicit profiling runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) profile_snapshot: Option<ProfileSnapshotReport>,
    /// Retained-memory breakdowns for the final project snapshot and selected profile artifacts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) memory: Vec<MemoryReport>,
}

impl AnalyzeReport {
    pub(crate) fn build(
        project: &Project,
        analysis_setup: AnalysisSetupReport,
        allocator: Option<AllocatorReport>,
        profile_snapshot: Option<&ProfileSnapshot>,
        include_profile_snapshot: bool,
        include_memory: bool,
    ) -> Self {
        let mut memory = include_memory
            .then(|| MemoryReport::capture(project))
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(profile_snapshot) = profile_snapshot {
            memory.extend(profile_snapshot.memory_snapshot_entries().map(
                |(descriptor, snapshot)| {
                    MemoryReport::capture_profile_snapshot(descriptor, snapshot)
                },
            ));
        }

        let build_profile_report = include_memory.then(|| {
            let profile_snapshot =
                profile_snapshot.expect("memory reporting should collect project build profile");
            let checkpoints = profile_snapshot
                .checkpoints(rg_project::BUILD_CHECKPOINTS_PROFILE_PATH)
                .expect("project build profile should record checkpoints");

            BuildProfileReport::capture(checkpoints)
        });

        Self {
            workspace_root: project.workspace().workspace_root().display().to_string(),
            project: ProjectReport::capture(project),
            analysis_setup,
            build_profile: build_profile_report,
            allocator,
            profile_snapshot: include_profile_snapshot
                .then(|| profile_snapshot.map(ProfileSnapshotReport::capture))
                .flatten(),
            memory,
        }
    }

    pub(crate) fn render_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub(crate) fn document(&self, options: ReportDocumentOptions) -> ReportDocument {
        let mut document = ReportDocument::builder("analyze")
            .title("rust-glancer analysis built")
            .section("project", |section| self.project.append_document(section));

        if options.include_memory
            && let Some(allocator) = &self.allocator
        {
            document = document.section("allocator", |section| allocator.append_document(section));
        }

        if options.include_memory
            && let Some(build_profile) = &self.build_profile
        {
            document = document.section("analysis_setup", |section| {
                self.analysis_setup.append_document(section);
            });

            let purge = options
                .include_memory
                .then(|| {
                    self.allocator
                        .as_ref()
                        .and_then(|allocator| allocator.purge.as_ref())
                })
                .flatten();

            document = document.section("build_profile", |section| {
                build_profile.append_document(section, purge);
            });
        }

        for memory in &self.memory {
            document = document.section(memory.section_key(), |section| {
                memory.append_document(section);
            });
        }

        if let Some(profile_snapshot) = &self.profile_snapshot {
            document = document.section("profile_snapshot", |section| {
                profile_snapshot.append_document(section);
            });
        }

        document.build()
    }
}
