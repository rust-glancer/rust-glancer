//! Analysis project snapshots and the storage used to serve them.
//!
//! `indexing` builds saved analysis, `change` applies saved source and workspace changes, and
//! `query` prepares request views. `storage` owns residency and artifact backing. `ProjectState`
//! keeps the source generation and phase databases shared by those operations.
//!
//! Several mechanisms here avoid repeating work, but they do not share one lifetime or failure
//! policy. Read them as three layers rather than one general cache:
//!
//! | Layer | Validity | Owner | Miss or mismatch |
//! | --- | --- | --- | --- |
//! | Saved analysis | Published [`ProjectGenerationId`] | The live [`Project`] | Build a private candidate and publish it only after success |
//! | Package backing | Workspace artifact identity and saved-source fingerprint | Package cache plus resident/offloaded phase stores | Reject the artifact and rebuild from source |
//! | Query working set | The exact saved project snapshot that produced it | One request | Reload artifacts and recompute solver state |
//!
//! These layers meet through explicit snapshots. Resident and offloaded packages remain the same
//! logical package slots, so query code does not branch on storage location. Request-owned decoded
//! payloads and solver arenas are released when the query finishes.
//!
//! When adding another shortcut, keep three facts next to its owner: what exact semantic state
//! makes reuse safe, who releases the retained data, and which ordinary path runs when it is
//! absent or malformed. Validation and reuse are operations, not extra public domain entities
//! unless callers genuinely need to reason about them on their own. A cache miss must change cost,
//! never query meaning.

mod change;
mod indexing;
mod memory;
mod profile;
mod project;
mod query;
mod selection;
mod state;
mod stats;
mod storage;

use std::sync::OnceLock;

pub use rg_body_ir::{
    CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceSelection,
};
pub use rg_def_map::{MacroExpansionLimitGroup, MacroExpansionLimitReport};

#[doc(hidden)]
pub use self::indexing::bench_support;
pub use self::{
    change::{AnalysisChangeSummary, ChangedFile, SavedFileChange},
    indexing::{
        AnalysisSurface, BodyPublication, BodyPublicationOutcome, IndexingPerformancePreference,
        PackageBatchSize, ProjectBuilder, SavedBodyBuildInputs, SavedBodyProducts, SplitIndexing,
        SplitIndexingMode, SplitIndexingProgress, SplitIndexingStage, StartupCacheLoad,
    },
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{BUILD_CHECKPOINTS, BuildProcessMemory, ProcessMemorySampler},
    project::Project,
    query::{DocumentSourceView, FileContext, ProjectSnapshot},
    state::ProjectGenerationId,
    stats::{MacroExpansionLimitBuildSummary, ProjectStats},
    storage::residency::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy},
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
