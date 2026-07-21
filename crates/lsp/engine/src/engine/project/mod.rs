//! Saved-project lifecycle owned by the LSP engine.
//!
//! The saved `Project` is the semantic baseline shared by every request, but it is never shared as
//! mutable state. `ProjectCoordinator` keeps it on the dispatcher lane, prepares query-selected
//! deferred data, and publishes source-generation changes in command order. Dirty-buffer overlays
//! and detached background indexing live in child modules because they borrow or clone that saved
//! baseline without becoming alternative owners of it.
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
    AnalysisSurface, DirtyOverlayScope, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint,
    ProjectSnapshot, SavedFileChange, SplitIndexingMode,
};
use rg_std::UniqueVec;
use rg_workspace::{CargoMetadataTarget, SysrootSources, WorkspaceMetadata};

use crate::{
    documents::DirtyDocumentSnapshot,
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

/// The only gateway to the saved analysis project on the engine thread.
///
/// Queries may borrow snapshots or ask for more deferred data to be materialized, while file
/// changes publish a new saved generation. Background indexing also returns here before it can be
/// merged. Keeping all three paths together is what makes generation checks meaningful and avoids
/// putting locks inside the semantic engine. A query that discovers newer disk contents also
/// records a stale-source latch here, so later queries return neutral results until the queued path
/// mutation succeeds.
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
            project: ProjectState::new(memory_control),
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
        Self::log_project_snapshot(snapshot, "initial early-start index");
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
    /// Source races are retried before this method returns, keeping the caller's foreground
    /// indexing activity live until a coherent replacement has actually been published. If files
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
        Self::log_project_snapshot(snapshot, "manual reindex");
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            stale_retries,
            "manual workspace reindex finished"
        );

        Ok(())
    }

    /// Apply one coalesced watcher/save/recovery batch to the saved project.
    ///
    /// `Project` decides whether each path needs a source rebuild or a Cargo graph rebuild. The
    /// coordinator owns publication, retry, and the deferred-indexing transition around that
    /// change. If another file changes while the candidate is being built, its path joins the batch
    /// before the whole mutation is retried. Exhausting the bounded retries rejects the command
    /// without replacing the last coherent generation, allowing later queued work to proceed.
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

        let mut paths = paths.into_iter().collect::<UniqueVec<_>>();
        let mut stale_retries = 0usize;
        let summary = loop {
            let result = self.mutate_saved_and_schedule_deferred_finish(|project| {
                project.apply_changes(paths.iter().map(SavedFileChange::new))
            });
            match result {
                Ok(summary) => break summary,
                Err(error) => {
                    let Some(stale_path) = Self::wait_for_stale_source_retry(
                        "project path changes",
                        stale_retries,
                        &error,
                    ) else {
                        return Err(error).context("apply project path changes");
                    };
                    paths.push(stale_path);
                    let scan_started = Instant::now();
                    match self.project.stale_saved_source_paths() {
                        Ok(stale_paths) => {
                            let discovered_paths = stale_paths.len();
                            for path in stale_paths {
                                paths.push(path);
                            }
                            tracing::debug!(
                                discovered_paths,
                                retry_path_count = paths.len(),
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
        };
        self.stale_source = None;
        let applied_changes = paths.len();
        let changed_files = summary.changed_files.len();
        let affected_packages = summary.affected_packages.len();
        let changed_crates = summary.changed_crates.len();

        tracing::info!(
            applied_changes,
            changed_files,
            affected_packages,
            changed_crates,
            stale_retries,
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

    /// Materialize one query-selected analysis surface without changing source generation.
    pub(super) fn materialize(&mut self, surface: AnalysisSurface<'_>) -> anyhow::Result<()> {
        self.project
            .materialize(surface)
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
        dirty_scope: DirtyOverlayScope,
        query: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.project
            .with_query_snapshot(dirty, dirty_scope, query)
            .context("run query with project snapshot")
    }

    /// Release request-scoped loads from both saved state and the cached dirty overlay.
    pub(super) fn release_query_memory(&mut self) {
        self.project.release_query_memory();
    }

    /// Return the path that keeps queries neutral until saved-path recovery succeeds.
    pub(super) fn stale_source(&self) -> Option<&Path> {
        self.stale_source.as_deref()
    }

    /// Stop queries at one observed saved/disk mismatch and schedule normal path-change recovery.
    ///
    /// Recovery deliberately enters the tail of the command queue. A native watcher batch already
    /// waiting there can run first, and adjacent path changes can coalesce. Most importantly, the
    /// query that discovered the mismatch no longer rebuilds the whole workspace while files are
    /// still changing.
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

        // Re-enter the same mutation stream as watcher and save changes. No response endpoint is
        // needed because the query has already degraded to its neutral result.
        let recovery = EngineCommand::ProjectPathsChanged {
            paths: vec![path],
            respond_to: None,
        };
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
}

#[cfg(test)]
mod tests;
