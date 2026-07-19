//! Saved-project lifecycle owned by the LSP engine.
//!
//! The saved `Project` is the semantic baseline shared by every request, but it is never shared as
//! mutable state. `ProjectCoordinator` keeps it on the dispatcher lane, prepares query-selected
//! deferred data, and publishes source-generation changes in command order. Dirty-buffer overlays
//! and detached background indexing live in child modules because they borrow or clone that saved
//! baseline without becoming alternative owners of it.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Sender},
    time::Instant,
};

use anyhow::Context as _;
use rg_lsp_proto::ServiceNotification;
use rg_project::{
    AnalysisSurface, Project, ProjectMemoryHooks, ProjectSnapshot, SavedFileChange,
    SplitIndexingMode,
};
use rg_workspace::{CargoMetadataTarget, SysrootSources, WorkspaceMetadata};

use crate::{
    documents::DirtyDocumentSnapshot,
    engine::{QueuedEngineCommand, command::DeferredIndexingResult},
    memory::{MemoryControl, ProjectMemoryReporter},
    project_stats::{ProjectStats, log_retained_memory},
    service::ServiceNotificationsSink,
};

mod config;
mod deferred;
mod state;

pub(crate) use self::config::ProjectConfiguration;
use self::{deferred::DeferredIndexingFinish, state::ProjectState};

/// The only gateway to the saved analysis project on the engine thread.
///
/// Queries may borrow snapshots or ask for more deferred data to be materialized, while file
/// changes publish a new saved generation. Background indexing also returns here before it can be
/// merged. Keeping all three paths together is what makes generation checks meaningful and avoids
/// putting locks inside the semantic engine.
#[derive(Debug)]
pub(super) struct ProjectCoordinator {
    project: ProjectState,
    deferred_indexing_finish: DeferredIndexingFinish,
    workspace_root: Option<PathBuf>,
    notifications: ServiceNotificationsSink,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

impl ProjectCoordinator {
    pub(super) fn new(
        sender: Sender<QueuedEngineCommand>,
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        let memory_hooks = Arc::new(ProjectMemoryReporter::new(memory_control.clone()));
        Self {
            project: ProjectState::new(memory_control),
            deferred_indexing_finish: DeferredIndexingFinish::new(sender),
            workspace_root: None,
            notifications,
            memory_hooks,
        }
    }

    /// Build the first queryable project, then finish its deferred portion in the background.
    ///
    /// Early-start indexing deliberately publishes a usable saved project before all Body IR is
    /// resident. The detached clone continues that work, and query-time materialization can fill
    /// individual files or crates on the saved side while the clone runs.
    pub(super) fn initialize(
        &mut self,
        root: PathBuf,
        configuration: ProjectConfiguration,
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        let configured_target = match configuration.cargo_metadata_config.target() {
            CargoMetadataTarget::Auto => "auto",
            CargoMetadataTarget::Triple(target) => target.as_str(),
        };
        tracing::info!(
            root = %root.display(),
            package_residency = configuration.package_residency_policy.config_name(),
            indexing_preference = configuration.indexing_preference.config_name(),
            cargo_target = configured_target,
            cargo_all_features = configuration.cargo_metadata_config.all_features_enabled(),
            cargo_no_default_features = configuration.cargo_metadata_config.no_default_features_enabled(),
            cargo_features = ?configuration.cargo_metadata_config.features(),
            cfg_test = configuration.workspace_lowering_config.is_cfg_test_enabled(),
            cfg_atoms = ?configuration.workspace_lowering_config.cfg_atoms(),
            "starting workspace indexing"
        );

        let manifest_path = root.join("Cargo.toml");
        if !manifest_path.exists() {
            anyhow::bail!(
                "workspace root {} does not contain Cargo.toml",
                root.display()
            );
        }

        // First turn client configuration and Cargo metadata into the project-domain workspace.
        let metadata_started = Instant::now();
        let metadata = configuration
            .cargo_metadata_config
            .load_metadata_with_target_cfg(&manifest_path)
            .context("load Cargo metadata")?;
        tracing::info!(
            package_count = metadata.metadata.packages.len(),
            elapsed_ms = metadata_started.elapsed().as_millis(),
            "cargo metadata finished"
        );

        let workspace = WorkspaceMetadata::lower(
            metadata.metadata,
            metadata.target_cfg,
            configuration.workspace_lowering_config.clone(),
        )
        .context("normalize Cargo metadata")?;
        let workspace_root = workspace.workspace_root().to_path_buf();
        let sysroot = if configuration.discover_sysroot {
            SysrootSources::discover(workspace.workspace_root())
        } else {
            None
        };
        match &sysroot {
            Some(sysroot) => {
                tracing::info!(
                    library_root = %sysroot.library_root().display(),
                    "sysroot sources discovered"
                );
            }
            None => {
                tracing::info!("sysroot sources unavailable");
            }
        }

        // Build only through the early-start boundary. This is the first state queries may see.
        let workspace = workspace.with_sysroot_sources(sysroot);
        let project = Project::builder(workspace)
            .workspace_lowering_config(configuration.workspace_lowering_config)
            .cargo_metadata_config(configuration.cargo_metadata_config)
            .indexing_preference(configuration.indexing_preference)
            .split_indexing_mode(SplitIndexingMode::EarlyStart)
            .package_residency_policy(configuration.package_residency_policy)
            .memory_hooks(Arc::clone(&self.memory_hooks))
            .build()
            .context("build LSP analysis project")?;
        // Publish the saved project before starting detached work. From this point on, any later
        // source generation makes this detached result stale.
        self.workspace_root = Some(workspace_root.clone());
        let detached = project.detach_split_indexing();
        let generation = self.project.replace_saved(project);
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow initialized project snapshot")?;
        Self::log_project_snapshot(snapshot, "initial early-start index");
        tracing::info!(
            workspace_root = %workspace_root.display(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace early-start indexing finished"
        );
        self.deferred_indexing_finish
            .start_initial(generation, detached);

        Ok(())
    }

    /// Rebuild the whole saved workspace and schedule deferred work for the new generation.
    pub(super) fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        let started = Instant::now();

        tracing::info!("manual workspace reindex started");
        self.mutate_saved_and_schedule_deferred_finish(Project::reindex_workspace)
            .context("reindex workspace")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow reindexed project snapshot")?;
        Self::log_project_snapshot(snapshot, "manual reindex");
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "manual workspace reindex finished"
        );

        Ok(())
    }

    /// Apply one coalesced watcher/save batch to the saved project.
    ///
    /// `Project` decides whether each path needs a source rebuild or a Cargo graph rebuild. The
    /// coordinator only owns the publication and deferred-indexing transition around that change.
    pub(super) fn project_paths_changed(&mut self, paths: Vec<PathBuf>) -> anyhow::Result<()> {
        let started = Instant::now();

        tracing::info!(path_count = paths.len(), "processing project path changes");
        if paths.is_empty() {
            tracing::info!(
                applied_changes = 0usize,
                changed_files = 0usize,
                affected_packages = 0usize,
                changed_crates = 0usize,
                elapsed_ms = started.elapsed().as_millis(),
                "project path reindex finished"
            );
            return Ok(());
        }

        let applied_changes = paths.len();
        let summary = self
            .mutate_saved_and_schedule_deferred_finish(|project| {
                project.apply_changes(paths.into_iter().map(SavedFileChange::new))
            })
            .context("apply project path changes")?;
        let changed_files = summary.changed_files.len();
        let affected_packages = summary.affected_packages.len();
        let changed_crates = summary.changed_crates.len();

        tracing::info!(
            applied_changes,
            changed_files,
            affected_packages,
            changed_crates,
            elapsed_ms = started.elapsed().as_millis(),
            "project path reindex finished"
        );
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow changed project snapshot")?;
        Self::log_project_snapshot(snapshot, "after project path changes");

        Ok(())
    }

    /// Reconcile one background finish on the same lane that owns saved generations.
    pub(super) fn deferred_indexing_finished(
        &mut self,
        generation: u64,
        result: DeferredIndexingResult,
    ) {
        let current_generation_finished =
            self.deferred_indexing_finish
                .finish_returned(&mut self.project, generation, result);
        if !current_generation_finished {
            return;
        }

        self.send_deferred_indexing_finished();
        if let Ok(snapshot) = self.project.saved_snapshot() {
            Self::log_project_snapshot(snapshot, "deferred indexing finish");
        }
    }

    /// Materialize one query-selected analysis surface without changing source generation.
    pub(super) fn materialize(&mut self, surface: AnalysisSurface<'_>) -> anyhow::Result<()> {
        self.project
            .mutate_saved_preserving_generation(|project| {
                project.split_indexing().materialize(surface)
            })
            .context("materialize analysis surface")
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.project
            .saved_snapshot()
            .context("borrow project snapshot")
    }

    /// Run a query against saved state or a disposable single-file dirty overlay.
    pub(super) fn with_query_snapshot<T>(
        &mut self,
        dirty: Option<&DirtyDocumentSnapshot>,
        query: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.project
            .with_query_snapshot(dirty, query)
            .context("run query with project snapshot")
    }

    /// Release request-scoped loads from both saved state and the cached dirty overlay.
    pub(super) fn release_query_memory(&mut self) {
        self.project.release_query_memory();
    }

    /// Rebuild the saved generation before retrying one query against a stale source.
    pub(super) fn recover_after_stale_source(&mut self, label: &'static str, path: &Path) -> bool {
        if !self.project.is_initialized() {
            return false;
        }

        tracing::warn!(
            label,
            path = %path.display(),
            "analysis query observed a stale source generation; rebuilding before one retry"
        );
        match self.mutate_saved_and_schedule_deferred_finish(|project| {
            project
                .reindex_workspace()
                .context("rebuild stale source generation")
        }) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    label,
                    path = %path.display(),
                    error = %format!("{error:#}"),
                    "stale source generation could not be rebuilt"
                );
                false
            }
        }
    }

    /// Repair invalid package artifacts after returning an empty result for the failed query.
    pub(super) fn recover_after_query_cache_failure(&mut self, label: &'static str) {
        if !self.project.is_initialized() {
            tracing::warn!(
                label,
                "analysis query hit invalid package cache before project initialization"
            );
            return;
        }

        let started = Instant::now();
        tracing::warn!(
            label,
            "analysis query hit invalid package cache; rebuilding cache before next command"
        );

        match self.mutate_saved_and_schedule_deferred_finish(|project| {
            project.recover_after_cache_load_failure()
        }) {
            Ok(()) => {
                let snapshot = self
                    .project
                    .saved_snapshot()
                    .expect("project should remain initialized after cache recovery");
                Self::log_project_snapshot(snapshot, "after package cache recovery");
                tracing::info!(
                    label,
                    elapsed_ms = started.elapsed().as_millis(),
                    "package cache recovery finished"
                );
            }
            Err(error) => {
                tracing::error!(
                    label,
                    error = %format!("{error:#}"),
                    "package cache recovery failed"
                );
            }
        }
    }

    /// Run one saved-project mutation and reconcile the background finish afterward.
    ///
    /// A detached clone may now describe an older generation. The deferred controller either
    /// starts work for the remaining saved state or records one restart after the in-flight clone
    /// returns, keeping peak memory bounded to one detached project.
    fn mutate_saved_and_schedule_deferred_finish<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let result = self.project.mutate_saved(mutation);
        if self.project.is_initialized() {
            self.deferred_indexing_finish
                .saved_project_changed(&self.project);
        }
        result
    }

    fn send_deferred_indexing_finished(&self) {
        let Some(root) = &self.workspace_root else {
            tracing::warn!("deferred indexing finished before workspace root was recorded");
            return;
        };

        self.notifications
            .send(ServiceNotification::DeferredIndexingFinished { root: root.clone() });
    }

    /// Log the retained project shape after a saved-state transition.
    fn log_project_snapshot(snapshot: ProjectSnapshot<'_>, label: &'static str) {
        ProjectStats::capture(snapshot).log_info(label);
        log_retained_memory(snapshot, label);
    }
}
