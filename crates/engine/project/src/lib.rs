//! Analysis project snapshots and the storage used to serve them.
//!
//! Several mechanisms here avoid repeating work, but they do not share one lifetime or failure
//! policy. Read them as four layers rather than one general cache:
//!
//! | Layer | Validity | Owner | Miss or mismatch |
//! | --- | --- | --- | --- |
//! | Saved analysis | Published [`ProjectGenerationId`] | The live [`Project`] | Build a private candidate and publish it only after success |
//! | Package backing | Workspace artifact identity and saved-source fingerprint | Package cache plus resident/offloaded phase stores | Reject the artifact and rebuild from source |
//! | Dirty analysis | Saved base generation, dirty text identity, and requested package scope | The frontend's bounded overlay cache | Rebuild the disposable overlay |
//! | Query working set | The exact project snapshot that produced it | One request, with a short handoff from dirty rebuild to its query | Reload artifacts and recompute solver state |
//!
//! These layers meet through explicit snapshots. A dirty overlay derives analysis from a saved
//! generation but does not publish source fingerprints, write artifacts, or restore saved
//! residency. Resident and offloaded packages remain the same logical package slots, so query code
//! does not branch on storage location. Request-owned decoded payloads and solver sessions may be
//! retained just long enough for the query following a dirty rebuild, then are dropped together.
//!
//! When adding another shortcut, keep three facts next to its owner: what exact semantic state
//! makes reuse safe, who releases the retained data, and which ordinary path runs when it is
//! absent or malformed. Validation and reuse are operations, not extra public domain entities
//! unless callers genuinely need to reason about them on their own. A cache miss must change cost,
//! never query meaning.

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
    profile::{BUILD_CHECKPOINTS, BuildProcessMemory, ProcessMemorySampler},
    project::{
        AnalysisChangeSummary, AnalysisSurface, ChangedFile, DetachedSplitIndexing,
        DirtyFileChange, DirtyOverlayScope, FileContext, FinishedSplitIndexing, Project,
        ProjectBuilder, ProjectGenerationId, ProjectSnapshot, ProjectStats, SavedFileChange,
        SplitIndexing, SplitIndexingMode, StartupCacheLoad,
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
            descriptors.extend_from_slice(rg_ty::profile_descriptors());
            descriptors
        })
        .as_slice()
}
