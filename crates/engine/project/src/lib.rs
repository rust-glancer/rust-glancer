pub(crate) mod cache;
mod indexing;
mod memory;
mod profile;
mod project;
mod residency;

pub use rg_def_map::DefMapFinalizationStats;

pub use self::{
    indexing::IndexingPerformancePreference,
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{
        BuildCheckpoint, BuildProcessMemory, BuildProfile, BuildProfileStage,
        BuildStageMemorySnapshot, CacheProbeProfile, ProcessMemorySampler,
    },
    project::{
        AnalysisChangeSummary, ChangedFile, DirtyFileChange, FileContext, Project, ProjectBuild,
        ProjectBuilder, ProjectSnapshot, ProjectStats, SavedFileChange, StartupCacheLoad,
    },
    residency::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy},
};

#[cfg(test)]
pub mod testonly;

#[cfg(test)]
mod tests;
