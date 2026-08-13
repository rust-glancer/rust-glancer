//! Ownership and publication boundary for one analysis project.
//!
//! The host's long-lived `Project` represents one successfully published saved-source generation.
//! Saved changes are applied to a private candidate and replace that generation only after
//! validation succeeds. Source overrides use a disposable clone derived from exact
//! `CapturedSource` values already known to the saved generation; that clone never publishes
//! fingerprints or becomes another saved-state owner.

mod build;
pub(crate) mod loading;
pub(crate) mod offloading;
mod package_set;
mod reference_search;
mod snapshot;
mod source_overrides;
mod split_indexing;
pub(crate) mod state;
mod stats;
pub(crate) mod subset;
pub(crate) mod txn;
pub(crate) mod update;

use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_parse::FileId;
use rg_source::CapturedSource;
use rg_workspace::WorkspaceMetadata;

use self::state::ProjectState;
use crate::{
    indexing::IndexingPerformancePreference,
    residency::{PackageResidency, PackageResidencyPlan},
};
use rg_std::MemorySize;

pub use self::state::ProjectGenerationId;
pub use self::{
    build::{ProjectBuilder, SplitIndexingMode, StartupCacheLoad},
    snapshot::ProjectSnapshot,
    source_overrides::SourceOverrideScope,
    split_indexing::{
        AnalysisSurface, DetachedSplitIndexing, FinishedSplitIndexing, SplitIndexing,
    },
    stats::ProjectStats,
};

/// Mutable owner for the current analysis state.
///
/// `Project` is the host-facing state container: it accepts saved file changes, refreshes the
/// derived phase databases, and hands out immutable snapshots for queries.
///
/// The main project intentionally follows a rebuild-on-save model. Captured source overrides are
/// handled in disposable derived projects so saved-state fingerprints, package cache artifacts,
/// and residency decisions remain tied to committed source files.
#[derive(Debug, Clone, MemorySize)]
pub struct Project {
    pub(crate) state: ProjectState,
    // Package artifacts describe the saved project. Preserve override provenance separately so a
    // derived project cannot use those artifacts to validate state inherited from another derived
    // project.
    has_source_overrides: bool,
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

    /// Returns the identity of the successfully published saved-source generation.
    pub fn generation_id(&self) -> ProjectGenerationId {
        self.state.generation_id()
    }

    /// Returns the normalized workspace metadata this project was built from.
    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.state.workspace()
    }

    /// Returns package residency decisions for this project.
    pub fn package_residency_plan(&self) -> &PackageResidencyPlan {
        self.state.package_residency_plan()
    }

    /// Returns the indexing speed/memory trade-off used by this project.
    pub fn indexing_preference(&self) -> IndexingPerformancePreference {
        self.state.indexing_preference
    }

    /// Returns coarse status counters without exposing raw phase databases.
    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    /// Returns whether an analysis error came from disposable package-cache storage.
    pub fn is_recoverable_cache_load_failure(error: &anyhow::Error) -> bool {
        ProjectState::is_recoverable_cache_load_failure(error)
    }

    /// Returns the saved path whose current disk bytes no longer match a frozen source revision.
    pub fn stale_source_path(error: &anyhow::Error) -> Option<&Path> {
        error.chain().find_map(|cause| {
            cause
                .downcast_ref::<rg_source::SourceError>()
                .and_then(rg_source::SourceError::stale_path)
        })
    }

    /// Finds all known saved sources that no longer match this published generation.
    ///
    /// Incremental rebuild recovery uses this only after one candidate has already reported a
    /// source race. Scanning then turns a settled multi-file edit into one retry without adding a
    /// full source-tree scan to ordinary saves.
    pub fn stale_saved_source_paths(&self) -> Result<Vec<PathBuf>, rg_source::SourceError> {
        self.state.parse_db().source_inventory().stale_saved_paths()
    }

    /// Builds a private candidate and publishes it as a new generation only after success.
    ///
    /// Candidate internals remain unreachable while `build` runs. Assigning the public generation
    /// id immediately before the state swap makes that swap the single publication point: failed
    /// work cannot advance live generation identity or leave partially rebuilt phase databases in
    /// the project.
    fn try_publish_generation<T>(
        &mut self,
        build: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.try_publish_generation_when(build, |_| true)
    }

    /// Builds a candidate but publishes it only when its output describes a real state change.
    fn try_publish_generation_when<T>(
        &mut self,
        build: impl FnOnce(&mut Project) -> anyhow::Result<T>,
        should_publish: impl FnOnce(&T) -> bool,
    ) -> anyhow::Result<T> {
        let mut candidate = self.clone();
        let output = build(&mut candidate)
            .context("while attempting to build project generation candidate")?;
        if !should_publish(&output) {
            return Ok(output);
        }

        candidate.state.generation_id = ProjectGenerationId::fresh();
        self.state = candidate.state;
        Ok(output)
    }

    /// Rebuilds the project from source and rewrites offloadable package cache artifacts.
    pub fn recover_after_cache_load_failure(&mut self) -> anyhow::Result<()> {
        self.try_publish_generation(|candidate| {
            offloading::ResidencyApplication::failure_recovery(&mut candidate.state).context(
                "while attempting to recover analysis project after package cache load failed",
            )
        })
    }

    /// Rebuilds the whole project from the current workspace graph and saved source files.
    pub fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        self.try_publish_generation(update::reindex_workspace)
    }

    /// Returns the split-indexing control surface for deferred analysis work.
    pub fn split_indexing(&mut self) -> SplitIndexing<'_> {
        SplitIndexing::new(self)
    }

    /// Clone this project into an owned background-finish handle.
    ///
    /// Early-start indexing lets the saved project become queryable while deferred payloads are
    /// still missing. Background completion must run on a clone so it cannot block the command
    /// loop, but callers should not receive that clone as a general-purpose `Project`. Returning a
    /// narrow handle keeps the only supported detached operation explicit: finish deferred indexing
    /// and later merge the result back into saved state.
    //
    // TODO: Make project snapshots cheap to detach, especially parse state, so background
    // completion does not have to clone large parse arenas on the caller thread.
    pub fn detach_split_indexing(&self) -> DetachedSplitIndexing {
        DetachedSplitIndexing::new(self.clone())
    }

    /// Applies one saved file replacement and refreshes derived analysis state.
    ///
    /// A path that disappeared before processing is ignored. Filesystem watchers can observe the
    /// old side of a rename after the new saved state is already on disk, and that stale event must
    /// not prevent other paths in the same batch from being applied.
    pub fn apply_change(
        &mut self,
        change: SavedFileChange,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        self.apply_changes([change])
    }

    /// Applies existing saved file replacements as one coherent project update.
    pub fn apply_changes(
        &mut self,
        changes: impl IntoIterator<Item = SavedFileChange>,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        let mut canonical_changes = std::collections::BTreeMap::new();

        for change in changes {
            let change = match change {
                SavedFileChange::FsPath(path) => {
                    let canonical_path = match path.canonicalize() {
                        Ok(path) => path,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            // We intentionally do not care about deleted module files. Valid Rust
                            // removes or changes the surviving `mod foo;` declaration, and saving
                            // that file rebuilds the graph. If the declaration still names the
                            // deleted file, keeping the previous analysis is good enough.
                            continue;
                        }
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!(
                                    "while attempting to canonicalize changed file {}",
                                    path.display()
                                )
                            });
                        }
                    };
                    SavedFileChange::FsPath(canonical_path)
                }
                SavedFileChange::Captured(source) => SavedFileChange::Captured(source),
            };
            let path = change.path().to_path_buf();
            match canonical_changes.entry(path) {
                Entry::Vacant(entry) => {
                    entry.insert(change);
                }
                Entry::Occupied(mut entry) => {
                    // Exact captured bytes are the stronger input boundary. A repeated filesystem
                    // path must not replace them with a later recapture, while a later captured
                    // value for the same path intentionally supersedes the first.
                    if change.captured_source().is_some() || entry.get().captured_source().is_none()
                    {
                        entry.insert(change);
                    }
                }
            }
        }

        // Watcher and editor notifications can name the same file through separate paths. The map
        // removes canonical aliases while preferring the latest exact captured value for a path.
        let canonical_changes = canonical_changes.into_values().collect::<Vec<_>>();

        if canonical_changes.is_empty() {
            return Ok(AnalysisChangeSummary::default());
        }

        let application = self
            .try_publish_generation_when(
                move |candidate| update::apply_canonical_changes(candidate, canonical_changes),
                |application| matches!(application, update::ProjectChangeApplication::Applied(_)),
            )
            .context("while attempting to apply saved file changes")?;
        match application {
            update::ProjectChangeApplication::Unchanged => Ok(AnalysisChangeSummary::default()),
            update::ProjectChangeApplication::Applied(summary) => Ok(summary),
        }
    }

    /// Captures editor text only when the path belongs to this project generation.
    pub fn capture_known_source(
        &self,
        path: &Path,
        text: impl Into<std::sync::Arc<str>>,
    ) -> Option<CapturedSource> {
        self.state
            .parse_db()
            .source_inventory()
            .capture_known(path, text)
    }

    /// Builds a disposable analysis project from captured source overrides.
    ///
    /// Inputs outside the selected source generation are ignored. Matching inputs keep their exact
    /// bytes available for lazy reads but need no derived state; if every known input matches, this
    /// returns `None`.
    ///
    /// The returned project is intentionally detached from saved-state cache lifecycle: override
    /// rebuilds keep rebuilt package payloads resident and never refresh source fingerprints or
    /// package artifacts. Callers can query the returned snapshot and then drop it.
    pub fn derive_with_source_overrides(
        &self,
        scope: SourceOverrideScope,
        sources: impl IntoIterator<Item = CapturedSource>,
    ) -> anyhow::Result<Option<Project>> {
        source_overrides::derive_project(self, scope, sources)
    }

    /// Drops state that read-only queries may lazily reconstruct from durable backing data.
    ///
    /// Saved-project transactions own their offloaded package payloads, so those die with the
    /// request. A source-override project also retains the loaders and solver sessions created by
    /// its rebuild until the matching query finishes; clearing the query cache drops that state as
    /// one unit.
    ///
    /// Source-coordinate conversion is different again: it repopulates line-index cells inside the
    /// project itself, and the owner has to reset those cells after the request settles.
    pub fn release_query_memory(&mut self) {
        let offloadable_packages = self
            .state
            .package_residency
            .packages()
            .iter()
            .enumerate()
            .filter_map(|(package_idx, residency)| {
                (*residency == PackageResidency::Offloadable).then_some(package_idx)
            })
            .collect::<Vec<_>>();

        self.state
            .parse
            .offload_line_indexes_for_packages(&offloadable_packages);
        self.state.parse.evict_saved_source_text();
        self.state.clear_query_cache();
    }
}

/// One saved-project input selected before candidate rebuilding starts.
///
/// `FsPath` captures disk at the project boundary. `Captured` instead carries exact Rust text and
/// revision from an earlier event boundary; final validation still proves that disk has not
/// advanced before the candidate is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedFileChange {
    Captured(CapturedSource),
    FsPath(PathBuf),
}

impl SavedFileChange {
    pub fn captured(source: CapturedSource) -> Self {
        Self::Captured(source)
    }

    pub fn fs_path(path: impl AsRef<Path>) -> Self {
        Self::FsPath(path.as_ref().to_path_buf())
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Captured(source) => source.path(),
            Self::FsPath(path) => path,
        }
    }

    pub fn captured_source(&self) -> Option<&CapturedSource> {
        match self {
            Self::Captured(source) => Some(source),
            Self::FsPath(_) => None,
        }
    }
}

/// Summary of what one saved-file update touched.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct AnalysisChangeSummary {
    pub changed_files: Vec<ChangedFile>,
    pub affected_packages: Vec<PackageSlot>,
    pub changed_crates: Vec<CrateRef>,
}

impl AnalysisChangeSummary {
    fn is_empty(&self) -> bool {
        self.changed_files.is_empty()
            && self.affected_packages.is_empty()
            && self.changed_crates.is_empty()
    }
}

/// One known package-local source file that was reparsed in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub struct ChangedFile {
    pub package: PackageSlot,
    pub file: FileId,
}

/// Analysis-ready context for one filesystem path.
///
/// The same file can be reachable from more than one crate, for example when a package library
/// and binary both declare `mod shared;`. Unreachable parsed-cache files are intentionally omitted
/// by path lookups, because LSP queries need a current crate context to answer semantic questions.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct FileContext {
    pub package: PackageSlot,
    pub file: FileId,
    pub crates: Vec<CrateRef>,
}
