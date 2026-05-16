mod build;
mod dirty;
pub(crate) mod loading;
mod memsize;
pub(crate) mod offloading;
mod snapshot;
pub(crate) mod state;
mod stats;
pub(crate) mod subset;
pub(crate) mod txn;
pub(crate) mod update;

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rg_def_map::{PackageSlot, TargetRef};
use rg_parse::FileId;
use rg_workspace::WorkspaceMetadata;

use self::state::ProjectState;
use crate::residency::PackageResidencyPlan;

pub use self::{
    build::{ProjectBuild, ProjectBuilder, StartupCacheLoad},
    dirty::DirtyFileChange,
    snapshot::ProjectSnapshot,
    stats::ProjectStats,
};

/// Mutable owner for the current analysis state.
///
/// `Project` is the LSP-facing state container: it accepts saved file changes, refreshes the
/// derived phase databases, and hands out immutable snapshots for queries.
///
/// The main project intentionally follows a rebuild-on-save model. Dirty editor buffers are handled
/// as temporary overlays so saved-state fingerprints, package cache artifacts, and residency
/// decisions remain tied to committed source files.
#[derive(Debug, Clone)]
pub struct Project {
    pub(crate) state: ProjectState,
}

impl Project {
    /// Starts configuring a fresh analysis project build.
    pub fn builder(workspace: WorkspaceMetadata) -> ProjectBuilder {
        ProjectBuilder::new(workspace)
    }

    /// Returns an immutable query view of the current project state.
    pub fn snapshot(&self) -> ProjectSnapshot<'_> {
        ProjectSnapshot { state: &self.state }
    }

    /// Returns the normalized workspace metadata this project was built from.
    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.state.workspace()
    }

    /// Returns package residency decisions for this project.
    pub fn package_residency_plan(&self) -> &PackageResidencyPlan {
        self.state.package_residency_plan()
    }

    /// Returns coarse status counters without exposing raw phase databases.
    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    /// Returns whether an analysis error came from disposable package-cache storage.
    pub fn is_recoverable_cache_load_failure(error: &anyhow::Error) -> bool {
        ProjectState::is_recoverable_cache_load_failure(error)
    }

    /// Rebuilds the project from source and rewrites offloadable package cache artifacts.
    pub fn recover_after_cache_load_failure(&mut self) -> anyhow::Result<()> {
        offloading::ResidencyApplication::failure_recovery(&mut self.state)
            .context("while attempting to recover analysis project after package cache load failed")
    }

    /// Rebuilds the whole project from the current workspace graph and saved source files.
    pub fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        update::reindex_workspace(self)
    }

    /// Applies one saved file replacement and refreshes derived analysis state.
    pub fn apply_change(
        &mut self,
        change: SavedFileChange,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        let path = change.path.canonicalize().with_context(|| {
            format!(
                "while attempting to canonicalize changed file {}",
                change.path.display()
            )
        })?;
        update::apply_change(self, SavedFileChange { path })
    }

    /// Builds an ephemeral analysis project from dirty editor buffers.
    ///
    /// The returned project is intentionally detached from saved-state cache lifecycle: dirty
    /// rebuilds keep rebuilt package payloads resident and never refresh source fingerprints or
    /// package artifacts. Callers can query the returned snapshot and then drop it.
    pub fn dirty_overlay(
        &self,
        changes: impl IntoIterator<Item = DirtyFileChange>,
    ) -> anyhow::Result<Option<Project>> {
        dirty::build_overlay(self, changes)
    }
}

/// One source file saved on disk.
///
/// The project treats the filesystem as the source of truth. This keeps save handling aligned
/// with the project's rebuild-on-save model and avoids retaining editor buffer text in analysis
/// caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFileChange {
    pub path: PathBuf,
}

impl SavedFileChange {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

/// Summary of what one saved-file change touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisChangeSummary {
    pub changed_files: Vec<ChangedFile>,
    pub affected_packages: Vec<PackageSlot>,
    pub changed_targets: Vec<TargetRef>,
}

/// One known package-local source file that was reparsed in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChangedFile {
    pub package: PackageSlot,
    pub file: FileId,
}

/// Analysis-ready context for one filesystem path.
///
/// The same file can be reachable from more than one target, for example when a package library
/// and binary both declare `mod shared;`. Unreachable parsed-cache files are intentionally omitted
/// by path lookups, because LSP queries need a current target context to answer semantic questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContext {
    pub package: PackageSlot,
    pub file: FileId,
    pub targets: Vec<TargetRef>,
}
