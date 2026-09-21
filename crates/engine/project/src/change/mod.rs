//! Applies saved source and workspace changes to a private project candidate.
//!
//! This layer decides what changed and which packages must be rebuilt. Indexing produces the new
//! analysis; the project publishes its generation only after the complete change succeeds.

mod input;
mod source;
mod workspace;
mod workspace_graph;

use std::collections::btree_map::Entry;

use anyhow::Context as _;
use rg_ir_model::PackageSlot;

use self::workspace_graph::WorkspaceGraphChanges;
use crate::{Project, StartupCacheLoad};

pub use self::input::{AnalysisChangeSummary, ChangedFile, SavedFileChange};

impl Project {
    /// Rebuilds the whole project from the current workspace graph and saved source files.
    ///
    /// Manual reindex is the explicit freshness boundary for passive Cargo build outputs. It
    /// bypasses startup cache restoration so the newly discovered artifact snapshot is rebuilt
    /// from source even when an older cached snapshot still validates on disk.
    pub fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        self.try_publish_generation(reindex_workspace)
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
                move |candidate| apply_canonical_changes(candidate, canonical_changes),
                |application| matches!(application, ProjectChangeApplication::Applied(_)),
            )
            .context("while attempting to apply saved file changes")?;
        match application {
            ProjectChangeApplication::Unchanged => Ok(AnalysisChangeSummary::default()),
            ProjectChangeApplication::Applied(summary) => Ok(summary),
        }
    }
}

/// Whether one canonical path batch changed the candidate's published project state.
enum ProjectChangeApplication {
    Unchanged,
    Applied(AnalysisChangeSummary),
}

fn reindex_workspace(project: &mut Project) -> anyhow::Result<()> {
    workspace::rebuild_workspace_graph(project, &[], StartupCacheLoad::Disabled)
        .context("while attempting to reindex analysis project from workspace root")?;
    Ok(())
}

/// Applies one nonempty canonicalized change batch to a private project candidate.
fn apply_canonical_changes(
    project: &mut Project,
    changes: Vec<SavedFileChange>,
) -> anyhow::Result<ProjectChangeApplication> {
    debug_assert!(
        !changes.is_empty(),
        "candidate updates should contain at least one canonical change",
    );

    let graph_changed = changes.iter().any(|change| {
        WorkspaceGraphChanges::check(
            project.state.workspace(),
            project.state.parse_db(),
            &project.state.cargo_metadata_config,
            change,
        ) == WorkspaceGraphChanges::Changed
    });

    if graph_changed {
        workspace::rebuild_workspace_graph(project, &changes, StartupCacheLoad::Enabled)
            .map(ProjectChangeApplication::Applied)
            .context("while attempting to rebuild analysis project after workspace change")
    } else {
        let summary = source::apply_source_changes(project, changes)
            .context("while attempting to apply saved source changes")?;
        if summary.is_empty() {
            Ok(ProjectChangeApplication::Unchanged)
        } else {
            Ok(ProjectChangeApplication::Applied(summary))
        }
    }
}

fn affected_packages(
    project: &Project,
    changed_files: &[ChangedFile],
    fallback_package_roots: &[PackageSlot],
) -> Vec<PackageSlot> {
    let changed_package_ids = changed_packages(changed_files, fallback_package_roots)
        .iter()
        .filter_map(|package_slot| {
            project
                .state
                .workspace()
                .packages()
                .get(package_slot.0)
                .map(|package| package.id.clone())
        })
        .collect::<Vec<_>>();

    project
        .state
        .workspace()
        .reverse_dependency_closure(&changed_package_ids)
        .into_iter()
        .map(PackageSlot)
        .collect()
}

fn changed_packages(
    changed_files: &[ChangedFile],
    fallback_package_roots: &[PackageSlot],
) -> Vec<PackageSlot> {
    let mut packages = changed_files
        .iter()
        .map(|changed_file| changed_file.package)
        .chain(fallback_package_roots.iter().copied())
        .collect::<Vec<_>>();
    packages.sort_by_key(|package| package.0);
    packages.dedup();
    packages
}
