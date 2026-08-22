//! Keeps the saved `Project` on the engine thread and changes it in command order.
//!
//! Queries borrow this project and may ask the coordinator to load package data that was left on
//! disk. Saves and file-watcher events build and publish a new project generation. Background
//! indexing may do expensive work elsewhere, but its result comes back to this module before it can
//! be merged. The project is therefore never shared as mutable state between those paths.
//!
//! A filesystem burst can keep changing after a watcher command reaches the engine. Project
//! construction reports that race as a typed stale-source error. The coordinator waits for another
//! quiet window and retries the same mutation on the lane; it never publishes the rejected
//! candidate or starts deferred work for it. Retries are bounded so continuous writers cannot keep
//! every later lifecycle command out of the single dispatcher lane forever.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_proto::ServiceNotification;
use rg_project::{
    AnalysisSurface, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint, ProjectSnapshot,
    SavedFileChange, SplitIndexingMode,
};
use rg_workspace::{CargoMetadataTarget, SysrootSources, WorkspaceMetadata};

use crate::{
    engine::{
        QueuedEngineCommand,
        command::{DeferredIndexingResult, EngineCommand},
    },
    memory::{MemoryControl, ProjectMemoryReporter},
    project_stats::{ProjectStats, log_retained_memory},
    service::ServiceNotificationsSink,
};

mod config;
mod deferred;
mod state;

pub(crate) use self::config::ProjectConfiguration;
use self::{deferred::DeferredIndexingFinish, state::ProjectState};

// A watcher batch can become stale again while its replacement project is being built. Waiting
// for one more quiet window avoids rebuilding in a tight loop while a checkout or agent edit is
// still landing.
const STALE_SOURCE_RETRY_DELAY: Duration = Duration::from_millis(600);
// One stale attempt is common when a watcher fires in the middle of a multi-file write. Three
// retries leave room for the burst to settle and for the retry scan to collect its remaining paths,
// while still putting a finite bound on how long one command can own the engine lane.
const MAX_STALE_SOURCE_RETRIES: usize = 3;

/// The only type allowed to access the saved project on the engine thread.
///
/// Queries, saved file changes, and completed background indexing all pass through this owner. It
/// can therefore compare generation ids before merging background work without putting locks in
/// the analysis databases. If a query discovers that disk has newer text, this owner also marks the
/// project stale so later queries stop until the queued file change publishes a replacement.
#[derive(Debug)]
pub(super) struct ProjectCoordinator {
    project: ProjectState,
    deferred_indexing_finish: DeferredIndexingFinish,
    command_sender: Sender<QueuedEngineCommand>,
    stale_source: Option<PathBuf>,
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
            project: ProjectState::new(),
            deferred_indexing_finish: DeferredIndexingFinish::new(sender.clone()),
            command_sender: sender,
            stale_source: None,
            workspace_root: None,
            notifications,
            memory_hooks,
        }
    }

    /// Build the first queryable project, then finish its deferred portion in the background.
    ///
    /// Early-start indexing deliberately publishes a usable saved project before all Body IR is
    /// resident. The detached clone continues that work, and query-time materialization can fill
    /// individual files or crates on the saved side while the clone runs. If workspace files keep
    /// changing during the initial build, only a coherent attempt is published.
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
            let sysroot = SysrootSources::discover(workspace.workspace_root());
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
            sysroot
        } else {
            tracing::info!("sysroot source discovery disabled");
            None
        };

        // Build only through the early-start boundary. This is the first state queries may see.
        let workspace = workspace.with_sysroot_sources(sysroot);
        let mut stale_retries = 0usize;
        let project = loop {
            let result = Project::builder(workspace.clone())
                .workspace_lowering_config(configuration.workspace_lowering_config.clone())
                .cargo_metadata_config(configuration.cargo_metadata_config.clone())
                .indexing_preference(configuration.indexing_preference)
                .split_indexing_mode(SplitIndexingMode::EarlyStart)
                .package_residency_policy(configuration.package_residency_policy)
                .memory_hooks(Arc::clone(&self.memory_hooks))
                .build();
            match result {
                Ok(project) => break project,
                Err(error) => {
                    let Some(_) = Self::wait_for_stale_source_retry(
                        "initial workspace indexing",
                        stale_retries,
                        &error,
                    ) else {
                        return Err(error).context("build LSP analysis project");
                    };
                    stale_retries = stale_retries.saturating_add(1);
                }
            }
        };
        // Publish the saved project before starting detached work. From this point on, any later
        // source generation makes this detached result stale.
        self.workspace_root = Some(workspace_root.clone());
        let detached = project.detach_split_indexing();
        let generation = self.project.replace_saved(project);
        self.stale_source = None;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow initialized project snapshot")?;
        Self::log_project_build(snapshot, "initial early-start index");
        tracing::info!(
            workspace_root = %workspace_root.display(),
            elapsed_ms = started.elapsed().as_millis(),
            stale_retries,
            "workspace early-start indexing finished"
        );
        if self
            .deferred_indexing_finish
            .start_initial(generation, detached)
        {
            self.send_deferred_indexing_started();
        }

        Ok(())
    }

    /// Rebuild the whole saved workspace and schedule deferred work for the new generation.
    ///
    /// Source races are retried before this method returns, keeping the caller's foreground update
    /// live until a coherent replacement has actually been published. If files
    /// keep changing through the retry limit, the old generation remains published and the caller
    /// receives the stale-source error; a later watcher batch can try again from the queue.
    pub(super) fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut stale_retries = 0usize;

        tracing::info!("manual workspace reindex started");
        loop {
            match self.mutate_saved_and_schedule_deferred_finish(Project::reindex_workspace) {
                Ok(()) => break,
                Err(error) => {
                    let Some(_) = Self::wait_for_stale_source_retry(
                        "manual workspace reindex",
                        stale_retries,
                        &error,
                    ) else {
                        return Err(error).context("reindex workspace");
                    };
                    stale_retries = stale_retries.saturating_add(1);
                }
            }
        }
        self.stale_source = None;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow reindexed project snapshot")?;
        Self::log_project_build(snapshot, "manual reindex");
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            stale_retries,
            "manual workspace reindex finished"
        );

        Ok(())
    }

    /// Publish exact captured Rust source and graph/discovery paths as one saved transaction.
    ///
    /// A stale path that already has a captured value rejects the proposal instead of silently
    /// replacing it with newer disk contents. Other project sources can still use the bounded
    /// settled-burst retry path while preserving every explicitly captured input.
    pub(super) fn saved_project_changes(
        &mut self,
        changes: Vec<SavedFileChange>,
    ) -> anyhow::Result<u64> {
        let started = Instant::now();
        let (summary, applied_changes, stale_retries) = self
            .apply_saved_project_changes("saved project changes", changes)
            .context("apply saved project changes")?;
        self.stale_source = None;

        tracing::info!(
            applied_changes,
            changed_files = summary.changed_files.len(),
            affected_packages = summary.affected_packages.len(),
            changed_crates = summary.changed_crates.len(),
            stale_retries,
            saved_project_generation = self.project.generation(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved project changes finished"
        );
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project snapshot")?;
        if summary.affected_packages.is_empty() {
            Self::log_project_snapshot(snapshot, "after saved project changes");
        } else {
            Self::log_project_build(snapshot, "after saved project changes");
        }

        Ok(self.project.generation())
    }

    /// Repair the one path whose saved revision a query proved stale.
    pub(super) fn recover_stale_source(&mut self, path: PathBuf) -> anyhow::Result<()> {
        self.saved_project_changes(vec![SavedFileChange::fs_path(path)])
            .map(|_| ())
            .context("recover stale saved source")
    }

    /// Apply one candidate batch, retrying only sources that were not explicitly captured.
    fn apply_saved_project_changes(
        &mut self,
        operation: &'static str,
        mut changes: Vec<SavedFileChange>,
    ) -> anyhow::Result<(rg_project::AnalysisChangeSummary, usize, usize)> {
        let captured_paths = changes
            .iter()
            .filter_map(|change| {
                change
                    .captured_source()
                    .map(|captured| captured.path().to_path_buf())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut stale_retries = 0usize;

        loop {
            let result = self.mutate_saved_and_schedule_deferred_finish(|project| {
                project.apply_changes(changes.clone())
            });
            match result {
                Ok(summary) => return Ok((summary, changes.len(), stale_retries)),
                Err(error) => {
                    let Some(stale_path) = Project::stale_source_path(&error) else {
                        return Err(error);
                    };
                    if captured_paths.contains(stale_path) {
                        return Err(error).context("captured source no longer matches disk");
                    }
                    let Some(stale_path) =
                        Self::wait_for_stale_source_retry(operation, stale_retries, &error)
                    else {
                        return Err(error);
                    };
                    changes.push(SavedFileChange::fs_path(stale_path));

                    let scan_started = Instant::now();
                    match self.project.stale_saved_source_paths() {
                        Ok(stale_paths) => {
                            let discovered_paths = stale_paths.len();
                            for path in stale_paths {
                                if !captured_paths.contains(&path) {
                                    changes.push(SavedFileChange::fs_path(path));
                                }
                            }
                            tracing::debug!(
                                discovered_paths,
                                retry_path_count = changes.len(),
                                elapsed_ms = scan_started.elapsed().as_millis(),
                                "collected settled source changes for project retry"
                            );
                        }
                        Err(scan_error) => {
                            // The reported stale path is still enough for a correct retry. Keep
                            // this best-effort coalescing failure from replacing the typed build
                            // error that selected the recovery path in the first place.
                            tracing::debug!(
                                error = %format!("{scan_error:#}"),
                                elapsed_ms = scan_started.elapsed().as_millis(),
                                "could not collect additional stale sources for project retry"
                            );
                        }
                    }
                    stale_retries = stale_retries.saturating_add(1);
                }
            }
        }
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

        // Reconciliation consumes the detached result on every path: merged, stale, failed, or
        // unknown. Purge only after that ownership boundary so the background project's Body IR
        // can actually leave allocator arenas instead of waiting for the next query cleanup.
        self.memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterDeferredIndexingFinish);
        if !current_generation_finished {
            return;
        }

        self.send_deferred_indexing_finished();
        if let Ok(snapshot) = self.project.saved_snapshot() {
            Self::log_project_snapshot(snapshot, "deferred indexing finish");
        }
    }

    /// Publish one resolved priority package without ending the lifecycle.
    pub(super) fn deferred_indexing_priority_package_finished(
        &mut self,
        generation: u64,
        finished: rg_project::FinishedSplitIndexing,
    ) {
        self.deferred_indexing_finish.priority_package_returned(
            &mut self.project,
            generation,
            finished,
        );
    }

    /// Update package scheduling priority from an editor open/close hint.
    pub(super) fn set_deferred_indexing_priority(&mut self, path: PathBuf, prioritized: bool) {
        self.deferred_indexing_finish
            .set_priority(&self.project, path, prioritized);
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.project
            .saved_snapshot()
            .context("borrow project snapshot")
    }

    /// Load deferred package data needed by the next query.
    pub(super) fn materialize_saved_project(
        &mut self,
        surface: AnalysisSurface<'_>,
    ) -> anyhow::Result<()> {
        self.project
            .materialize_saved_project(surface)
            .context("materialize saved analysis surface")
    }

    /// Drop source data that was loaded only for the request that just finished.
    pub(super) fn release_query_memory(&mut self) {
        self.project.release_query_memory();
    }

    /// Return the path that blocks new queries until saved-path recovery succeeds.
    pub(super) fn stale_source(&self) -> Option<&Path> {
        self.stale_source.as_deref()
    }

    /// Stop queries at one observed saved/disk mismatch and schedule normal path-change recovery.
    ///
    /// Recovery deliberately enters the tail of the command queue. A native watcher batch already
    /// waiting there can run first. Most importantly, the query that discovered the mismatch no
    /// longer rebuilds the whole workspace while files are still changing.
    pub(super) fn record_stale_source(&mut self, label: &'static str, path: &Path) {
        if self.stale_source.is_some() || !self.project.is_initialized() {
            return;
        }

        tracing::warn!(
            label,
            path = %path.display(),
            "analysis query observed a stale source generation; scheduling path recovery"
        );
        let path = path.to_path_buf();
        self.stale_source = Some(path.clone());

        // Recovery enters the tail of the same FIFO lane as saved transactions. No response
        // endpoint is needed because the query already returned an explicit source-changed error.
        let recovery = EngineCommand::RecoverStaleSource { path };
        if let Err(error) = self
            .command_sender
            .send(QueuedEngineCommand::new(recovery))
            .context("enqueue stale-source path recovery")
        {
            tracing::warn!(
                label,
                error = %format!("{error:#}"),
                "failed to schedule stale-source recovery"
            );
        }
    }

    /// Repair invalid package artifacts after returning an error for the failed query.
    pub(super) fn recover_after_package_cache_failure(&mut self, label: &'static str) {
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
                Self::log_project_build(snapshot, "after package cache recovery");
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
    /// returns, keeping peak memory bounded to one detached project. A rejected mutation changes no
    /// generation, so it must not schedule a misleading deferred finish.
    fn mutate_saved_and_schedule_deferred_finish<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let previous_generation = self.project.generation();
        let result = self.project.mutate_saved(mutation);
        // A rejected candidate leaves the published generation untouched. Starting deferred work
        // for it would produce a false completion notification while recovery is still pending.
        // Exact watcher replays are successful no-ops and likewise have no new generation to
        // finish.
        let deferred_started = result.is_ok()
            && self.project.is_initialized()
            && self.project.generation() != previous_generation
            && self
                .deferred_indexing_finish
                .saved_project_changed(&self.project);
        if deferred_started {
            self.send_deferred_indexing_started();
        }
        result
    }

    /// Recognize a source-consistency race and wait before rebuilding from the newer disk state.
    ///
    /// Only typed stale/existence errors qualify; ordinary build failures return `None` immediately.
    /// Sleeping on the engine lane is intentional here: the foreground RPC remains pending when
    /// there is one, and no queued request can observe the rejected candidate between retries. The
    /// retry limit prevents that consistency guarantee from turning continuous writes into
    /// permanent ownership of the lane.
    fn wait_for_stale_source_retry(
        operation: &'static str,
        retry: usize,
        error: &anyhow::Error,
    ) -> Option<PathBuf> {
        let path = Project::stale_source_path(error)?.to_path_buf();
        if retry >= MAX_STALE_SOURCE_RETRIES {
            tracing::warn!(
                operation,
                attempts = retry.saturating_add(1),
                retry_limit = MAX_STALE_SOURCE_RETRIES,
                path = %path.display(),
                "saved project kept changing; stale-source retry limit reached"
            );
            return None;
        }

        tracing::info!(
            operation,
            retry = retry.saturating_add(1),
            path = %path.display(),
            delay_ms = STALE_SOURCE_RETRY_DELAY.as_millis(),
            "saved project changed during rebuild; waiting to retry"
        );
        thread::sleep(STALE_SOURCE_RETRY_DELAY);
        Some(path)
    }

    fn send_deferred_indexing_started(&self) {
        let Some(root) = &self.workspace_root else {
            tracing::warn!("deferred indexing started before workspace root was recorded");
            return;
        };

        self.notifications
            .send(ServiceNotification::DeferredIndexingStarted { root: root.clone() });
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

    /// Log project shape plus diagnostics produced by a newly completed def-map build.
    fn log_project_build(snapshot: ProjectSnapshot<'_>, label: &'static str) {
        Self::log_project_snapshot(snapshot, label);
        ProjectStats::log_macro_expansion_limit(snapshot.macro_expansion_limit_summary(), label);
    }
}

#[cfg(test)]
mod tests;
