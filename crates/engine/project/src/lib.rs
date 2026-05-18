pub(crate) mod cache;
mod memory;
mod profile;
mod project;
mod residency;

pub use self::{
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
mod tests;
