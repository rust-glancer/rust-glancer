//! Saved project publication and scoped access to Ajika-backed analysis.

mod change;
mod indexing;
mod memory;
mod profile;
mod project;
mod project_queries;
mod query;
mod state;
mod stats;

use std::sync::OnceLock;

pub use rg_body_ir::{
    CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceSelection,
};
pub use rg_def_map::{MacroExpansionLimitGroup, MacroExpansionLimitReport};

pub use self::{
    change::{AnalysisChangeSummary, ChangedFile, SavedFileChange},
    indexing::ProjectBuilder,
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{BUILD_CHECKPOINTS, BuildProcessMemory, ProcessMemorySampler},
    project::Project,
    query::{DocumentSourceView, FileContext, ProjectSnapshot},
    state::ProjectGenerationId,
    stats::{MacroExpansionLimitBuildSummary, ProjectStats},
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
            descriptors.extend_from_slice(rg_body_ir::profile_descriptors());
            descriptors.extend_from_slice(rg_def_map::profile_descriptors());
            descriptors.extend_from_slice(rg_ty::profile_descriptors());
            descriptors
        })
        .as_slice()
}
