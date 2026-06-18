pub(crate) mod cache;
mod indexing;
mod memory;
mod profile;
mod project;
mod residency;

use std::sync::OnceLock;

pub use self::{
    indexing::IndexingPerformancePreference,
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{
        BUILD_CHECKPOINTS_PROFILE_PATH, BUILD_PROFILE_SCOPE, BuildProcessMemory,
        ProcessMemorySampler,
    },
    project::{
        AnalysisChangeSummary, ChangedFile, DirtyFileChange, FileContext, Project, ProjectBuilder,
        ProjectSnapshot, ProjectStats, SavedFileChange, StartupCacheLoad,
    },
    residency::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy},
};

#[cfg(test)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub fn profile_descriptors() -> &'static [rg_profile::ProfileDescriptor] {
    static DESCRIPTORS: OnceLock<Vec<rg_profile::ProfileDescriptor>> = OnceLock::new();

    DESCRIPTORS
        .get_or_init(|| {
            let mut descriptors = Vec::new();
            descriptors.extend_from_slice(profile::profile_descriptors());
            descriptors.extend_from_slice(rg_def_map::profile_descriptors());
            descriptors
        })
        .as_slice()
}
