//! Client-facing projections of Rust Glancer's workspace lifecycle.
//!
//! One engine lifecycle is reported through three overlapping protocols. They are intentionally
//! additive because each client consumes a different part of the picture:
//!
//! - Standard LSP work-done progress presents a bounded indexing operation. Zed renders this as
//!   `Indexing <workspace>`, and other LSP clients can render it without Rust Glancer knowledge.
//! - Rust Glancer's private notifications preserve the richer VS Code status bar.
//! - Rust-analyzer's `experimental/serverStatus` extension presents persistent process-wide health.
//!   Zed maps its `health` and `message` fields into the Language Servers menu; Zed does not use its
//!   `quiescent` field as an indexing indicator. `compare-lsp` waits for healthy quiescence before
//!   measuring body-sensitive requests.
//!
//! `ClientStatusPublisher` is the facade for these projections. Engine orchestration reports
//! workspace events here and does not construct editor-specific payloads itself. Engine-originated
//! work-done operations such as Cargo diagnostics still enter through `NotificationsPublisher`,
//! but their standard LSP encoding also lives in `work_done_progress`.

mod rust_analyzer;
mod rust_glancer;
pub(crate) mod work_done_progress;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_lsp_proto::{DeferredIndexingOutcome, IndexingProgress, ProjectInitialization};
use tokio::sync::Mutex;
use tower_lsp_server::{Client as LspClient, gen_lsp_types::ClientCapabilities};

pub(crate) use self::rust_glancer::{ActiveWorkspaceState, ActiveWorkspaceStatus};
use self::work_done_progress::WorkspaceProgressState;

/// Client presentation features negotiated during LSP initialization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientStatusCapabilities {
    work_done_progress: bool,
    rust_analyzer_server_status: bool,
}

impl ClientStatusCapabilities {
    pub(crate) fn from_lsp_client_capabilities(capabilities: &ClientCapabilities) -> Self {
        Self {
            work_done_progress: work_done_progress::is_supported(capabilities),
            rust_analyzer_server_status: rust_analyzer::is_supported(capabilities),
        }
    }
}

/// Publishes the client views derived from all workspace-engine lifecycles.
///
/// Callers report domain events such as a workspace becoming ready. This type owns the shared
/// lifecycle state and decides which protocol projections need an update.
#[derive(Clone, Debug)]
pub(crate) struct ClientStatusPublisher {
    lsp_client: LspClient,
    capabilities: ClientStatusCapabilities,
    state: Arc<Mutex<ClientStatusState>>,
}

impl ClientStatusPublisher {
    pub(crate) fn new(lsp_client: LspClient, capabilities: ClientStatusCapabilities) -> Self {
        Self {
            lsp_client,
            capabilities,
            state: Arc::new(Mutex::new(ClientStatusState::default())),
        }
    }

    /// Start one foreground indexing operation unless that workspace is already updating.
    pub(crate) async fn workspace_indexing(&self, root: &Path) {
        let mut state = self.state.lock().await;
        if matches!(
            state.workspaces.get(root),
            Some(WorkspaceLifecycle::Indexing | WorkspaceLifecycle::Unavailable(_))
        ) {
            return;
        }

        state
            .workspaces
            .insert(root.to_path_buf(), WorkspaceLifecycle::Indexing);
        if self.capabilities.work_done_progress {
            state
                .workspace_progress
                .begin_foreground(&self.lsp_client, root)
                .await;
        }
        self.publish_rust_analyzer_status(&mut state).await;
    }

    pub(crate) async fn workspace_ready(&self, root: &Path) {
        self.finish_workspace_indexing(root, WorkspaceLifecycle::Ready, "Finished")
            .await;
    }

    /// Merge the engine's startup snapshot before declaring its workspace ready.
    ///
    /// The response can overtake background notifications or arrive after the worker has already
    /// finished. Fill in missing state without replacing a notification for the same generation.
    /// This keeps quiescence truthful in both orders, including startup with no background work.
    pub(crate) async fn workspace_initialized(
        &self,
        root: &Path,
        initialization: ProjectInitialization,
    ) {
        {
            let mut state = self.state.lock().await;
            if matches!(
                state.workspaces.get(root),
                Some(WorkspaceLifecycle::Unavailable(_))
            ) {
                return;
            }
            if state
                .deferred_indexing
                .get(root)
                .is_none_or(|deferred| deferred.generation() < initialization.generation)
            {
                let deferred = if initialization.has_deferred_indexing {
                    DeferredIndexingState::Running {
                        generation: initialization.generation,
                        progress: None,
                    }
                } else {
                    DeferredIndexingState::Idle {
                        generation: initialization.generation,
                    }
                };
                state.deferred_indexing.insert(root.to_path_buf(), deferred);
            }
        }
        self.workspace_ready(root).await;
    }

    pub(crate) async fn workspace_failed(&self, root: &Path, error: impl Into<Arc<str>>) {
        self.finish_workspace_indexing(root, WorkspaceLifecycle::Failed(error.into()), "Failed")
            .await;
    }

    /// Finish a workspace permanently so late callbacks from its process cannot restart progress.
    pub(crate) async fn workspace_unavailable(&self, root: &Path, error: impl Into<Arc<str>>) {
        self.finish_workspace_indexing(
            root,
            WorkspaceLifecycle::Unavailable(error.into()),
            "Failed",
        )
        .await;
    }

    /// Publish the workspace selected by document routing through Rust Glancer's private protocol.
    pub(crate) async fn active_workspace_changed(&self, status: Option<ActiveWorkspaceStatus>) {
        if let Some(status) = status {
            rust_glancer::active_workspace_changed(&self.lsp_client, &status).await;
        }
    }

    /// Mark background work while preserving the existing private VS Code notification.
    pub(crate) async fn deferred_indexing_started(&self, root: &Path, generation: u64) {
        {
            let mut state = self.state.lock().await;
            let workspace_is_available = !matches!(
                state.workspaces.get(root),
                Some(WorkspaceLifecycle::Unavailable(_))
            );
            if workspace_is_available {
                let starts_new_generation = match state.deferred_indexing.get(root) {
                    None => true,
                    Some(previous) => {
                        generation > previous.generation()
                            || generation == previous.generation() && previous.is_failed()
                    }
                };
                if starts_new_generation {
                    state.deferred_indexing.insert(
                        root.to_path_buf(),
                        DeferredIndexingState::Running {
                            generation,
                            progress: None,
                        },
                    );

                    if self.capabilities.work_done_progress
                        && matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
                    {
                        state
                            .workspace_progress
                            .begin_deferred(&self.lsp_client, root)
                            .await;
                    }
                    self.publish_rust_analyzer_status(&mut state).await;
                }
            }
        }

        // The private event is an independent compatibility contract. In particular, it is still
        // sent when a late engine callback no longer changes the aggregate lifecycle above.
        rust_glancer::deferred_indexing_started(&self.lsp_client, root).await;
    }

    /// Store and render progress only for the deferred generation that is still active.
    pub(crate) async fn deferred_indexing_progress(
        &self,
        root: &Path,
        generation: u64,
        progress: IndexingProgress,
    ) {
        let mut state = self.state.lock().await;
        {
            let Some(DeferredIndexingState::Running {
                generation: active_generation,
                progress: active_progress,
            }) = state.deferred_indexing.get_mut(root)
            else {
                return;
            };
            if *active_generation != generation || *active_progress == Some(progress) {
                return;
            }
            *active_progress = Some(progress);
        }

        if self.capabilities.work_done_progress
            && matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
        {
            state.workspace_progress.report(root, progress).await;
        }
    }

    pub(crate) async fn deferred_indexing_finished(
        &self,
        root: &Path,
        generation: u64,
        outcome: DeferredIndexingOutcome,
    ) {
        {
            let mut state = self.state.lock().await;
            let generation_is_active = state.deferred_indexing.get(root).is_some_and(|deferred| {
                matches!(
                    deferred,
                    DeferredIndexingState::Running {
                        generation: active_generation,
                        ..
                    } if *active_generation == generation
                )
            });
            if generation_is_active {
                let progress_message = match &outcome {
                    DeferredIndexingOutcome::Succeeded => {
                        // Retain the generation so a later initialization response cannot turn
                        // completed work back into a running operation.
                        state.deferred_indexing.insert(
                            root.to_path_buf(),
                            DeferredIndexingState::Idle { generation },
                        );
                        "Finished"
                    }
                    DeferredIndexingOutcome::Failed { message } => {
                        state.deferred_indexing.insert(
                            root.to_path_buf(),
                            DeferredIndexingState::Failed {
                                generation,
                                message: Arc::from(message.as_str()),
                            },
                        );
                        "Failed"
                    }
                };
                if self.capabilities.work_done_progress
                    && matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
                {
                    state
                        .workspace_progress
                        .finish(root, progress_message)
                        .await;
                }
                self.publish_rust_analyzer_status(&mut state).await;
            }
        }

        rust_glancer::deferred_indexing_finished(&self.lsp_client, root, &outcome).await;
    }

    async fn finish_workspace_indexing(
        &self,
        root: &Path,
        lifecycle: WorkspaceLifecycle,
        progress_message: &'static str,
    ) {
        let mut state = self.state.lock().await;
        if !matches!(lifecycle, WorkspaceLifecycle::Unavailable(_))
            && matches!(
                state.workspaces.get(root),
                Some(WorkspaceLifecycle::Unavailable(_))
            )
        {
            return;
        }

        state.workspaces.insert(root.to_path_buf(), lifecycle);
        state
            .workspace_progress
            .finish(root, progress_message)
            .await;
        let workspace_failed = matches!(
            state.workspaces.get(root),
            Some(WorkspaceLifecycle::Failed(_) | WorkspaceLifecycle::Unavailable(_))
        );
        if workspace_failed {
            state.deferred_indexing.remove(root);
        } else if matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
            && self.capabilities.work_done_progress
        {
            let progress = state
                .deferred_indexing
                .get(root)
                .and_then(DeferredIndexingState::running_progress);
            if let Some(progress) = progress {
                state
                    .workspace_progress
                    .begin_deferred(&self.lsp_client, root)
                    .await;
                if let Some(progress) = progress {
                    state.workspace_progress.report(root, progress).await;
                }
            }
        }
        self.publish_rust_analyzer_status(&mut state).await;
    }

    async fn publish_rust_analyzer_status(&self, state: &mut ClientStatusState) {
        if !self.capabilities.rust_analyzer_server_status {
            return;
        }

        let status = state.rust_analyzer_status();
        if state.last_rust_analyzer_status.as_ref() == Some(&status) {
            return;
        }

        rust_analyzer::publish(&self.lsp_client, &status).await;
        state.last_rust_analyzer_status = Some(status);
    }
}

#[derive(Debug, Default)]
struct ClientStatusState {
    workspaces: BTreeMap<PathBuf, WorkspaceLifecycle>,
    deferred_indexing: BTreeMap<PathBuf, DeferredIndexingState>,
    workspace_progress: WorkspaceProgressState,
    last_rust_analyzer_status: Option<self::rust_analyzer::StatusSnapshot>,
}

impl ClientStatusState {
    /// Collapse per-workspace state into rust-analyzer's server-wide health vocabulary.
    fn rust_analyzer_status(&self) -> self::rust_analyzer::StatusSnapshot {
        let failures = self
            .workspaces
            .iter()
            .filter_map(|(root, lifecycle)| match lifecycle {
                WorkspaceLifecycle::Failed(error) | WorkspaceLifecycle::Unavailable(error) => {
                    Some((root, error))
                }
                WorkspaceLifecycle::Indexing | WorkspaceLifecycle::Ready => None,
            })
            .collect::<Vec<_>>();
        let deferred_failures = self
            .deferred_indexing
            .iter()
            .filter_map(|(root, deferred)| match deferred {
                DeferredIndexingState::Failed { message, .. } => Some((root, message)),
                DeferredIndexingState::Idle { .. } | DeferredIndexingState::Running { .. } => None,
            })
            .collect::<Vec<_>>();
        let health = if !failures.is_empty() && failures.len() == self.workspaces.len() {
            rust_analyzer::Health::Error
        } else if failures.is_empty() && deferred_failures.is_empty() {
            rust_analyzer::Health::Ok
        } else {
            rust_analyzer::Health::Warning
        };
        let issue_count = failures.len() + deferred_failures.len();
        let message = failures
            .first()
            .map(|(root, error)| ("failed", root, error))
            .or_else(|| {
                deferred_failures
                    .first()
                    .map(|(root, error)| ("background indexing failed", root, error))
            })
            .map(|(description, root, error)| {
                let additional = issue_count.saturating_sub(1);
                let suffix = if additional == 0 {
                    String::new()
                } else {
                    format!(" (+{additional} more)")
                };
                format!(
                    "Workspace `{}` {description}: {error}{suffix}",
                    root.display()
                )
            });
        let foreground_work = self
            .workspaces
            .values()
            .any(|lifecycle| matches!(lifecycle, WorkspaceLifecycle::Indexing));
        let deferred_work = self
            .deferred_indexing
            .values()
            .any(|deferred| matches!(deferred, DeferredIndexingState::Running { .. }));

        rust_analyzer::StatusSnapshot {
            health,
            quiescent: !foreground_work && !deferred_work,
            message,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DeferredIndexingState {
    Idle {
        generation: u64,
    },
    Running {
        generation: u64,
        progress: Option<IndexingProgress>,
    },
    Failed {
        generation: u64,
        message: Arc<str>,
    },
}

impl DeferredIndexingState {
    fn generation(&self) -> u64 {
        match self {
            Self::Idle { generation }
            | Self::Running { generation, .. }
            | Self::Failed { generation, .. } => *generation,
        }
    }

    fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Preserve the distinction between no running operation and running without a first report.
    fn running_progress(&self) -> Option<Option<IndexingProgress>> {
        match self {
            Self::Running { progress, .. } => Some(*progress),
            Self::Idle { .. } | Self::Failed { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkspaceLifecycle {
    Indexing,
    Ready,
    Failed(Arc<str>),
    Unavailable(Arc<str>),
}

#[cfg(test)]
mod tests;
