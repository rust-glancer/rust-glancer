mod allocator;
mod def_map_finalization_stats;
mod memory;
mod package;
mod project;
mod stages;

use rg_project::{BuildProfile, DefMapFinalizationStats, Project};
use serde::Serialize;

pub(crate) use self::{
    allocator::{AllocatorPurgeReport, AllocatorReport},
    stages::AnalysisSetupReport,
};

use self::{
    def_map_finalization_stats::DefMapFinalizationStatsReport, memory::MemoryReport,
    project::ProjectReport, stages::BuildProfileReport,
};
use super::{CliMemoryStage, report::ReportDocument};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReportDocumentOptions {
    pub(crate) include_profile: bool,
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
    /// Optional counters and timings from def-map finalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) def_map_finalization_stats: Option<DefMapFinalizationStatsReport>,
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
        finalization_stats: Option<&DefMapFinalizationStats>,
        include_memory: bool,
        memory_stage: CliMemoryStage,
    ) -> Self {
        let memory = include_memory.then(|| match memory_stage {
            CliMemoryStage::Final => MemoryReport::capture(project),
            _ => build_profile
                .and_then(|profile| profile.stage_memory())
                .map(MemoryReport::capture_stage)
                .expect("selected build memory stage should be captured"),
        });

        Self {
            workspace_root: project.workspace().workspace_root().display().to_string(),
            project: ProjectReport::capture(project),
            analysis_setup,
            build_profile: build_profile.map(BuildProfileReport::capture),
            allocator,
            def_map_finalization_stats: finalization_stats
                .map(DefMapFinalizationStatsReport::capture),
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

        if options.include_profile {
            document = document.section("analysis_setup", |section| {
                self.analysis_setup.append_document(section);
            });
        }

        if options.include_memory
            && let Some(allocator) = &self.allocator
        {
            document = document.section("allocator", |section| allocator.append_document(section));
        }

        if (options.include_profile || options.include_memory)
            && let Some(build_profile) = &self.build_profile
        {
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

            if let Some(cache_probe) = &build_profile.cache_probe {
                document = document.section("cache_probe", |section| {
                    cache_probe.append_document(section);
                });
            }
        }

        if options.include_memory
            && let Some(memory) = &self.memory
        {
            document = document.section("memory", |section| memory.append_document(section));
        }

        if let Some(finalization_stats) = &self.def_map_finalization_stats {
            document = document.section("def_map_finalization_stats", |section| {
                finalization_stats.append_document(section);
            });
        }

        document.build()
    }
}
