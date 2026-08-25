//! Client-facing projections of Rust Glancer's workspace lifecycle.
//!
//! One engine lifecycle is reported through three overlapping protocols. They are intentionally
//! additive because each client consumes a different part of the picture:
//!
//! - Standard LSP work-done progress presents a bounded indexing operation. Zed renders this as
//!   `Indexing <workspace>`, and other LSP clients can render it without Rust Glancer knowledge.
//! - Rust Glancer's private notifications preserve the richer VS Code status bar and give
//!   `compare-lsp` an exact deferred-indexing barrier.
//! - Rust-analyzer's `experimental/serverStatus` extension presents persistent process-wide health.
//!   Zed maps its `health` and `message` fields into the Language Servers menu; Zed does not use its
//!   `quiescent` field as an indexing indicator.
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

use rg_lsp_proto::IndexingProgress;
use tokio::sync::Mutex;
use tower_lsp_server::{Client as LspClient, ls_types::ClientCapabilities};

pub(crate) use self::rust_glancer::{ActiveWorkspaceState, ActiveWorkspaceStatus};

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
                let previous_generation = state
                    .deferred_indexing
                    .get(root)
                    .map(|deferred| deferred.generation);
                let starts_new_generation = previous_generation
                    .is_none_or(|previous_generation| generation > previous_generation);
                if starts_new_generation {
                    state.deferred_indexing.insert(
                        root.to_path_buf(),
                        DeferredIndexingState {
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
                    if previous_generation.is_none() {
                        self.publish_rust_analyzer_status(&mut state).await;
                    }
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
            let Some(deferred) = state.deferred_indexing.get_mut(root) else {
                return;
            };
            if deferred.generation != generation || deferred.progress == Some(progress) {
                return;
            }
            deferred.progress = Some(progress);
        }

        if self.capabilities.work_done_progress
            && matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
        {
            state.workspace_progress.report(root, progress).await;
        }
    }

    pub(crate) async fn deferred_indexing_finished(&self, root: &Path, generation: u64) {
        {
            let mut state = self.state.lock().await;
            let generation_is_active = state
                .deferred_indexing
                .get(root)
                .is_some_and(|deferred| deferred.generation == generation);
            if generation_is_active {
                state.deferred_indexing.remove(root);
                if self.capabilities.work_done_progress
                    && matches!(state.workspaces.get(root), Some(WorkspaceLifecycle::Ready))
                {
                    state.workspace_progress.finish(root, "Finished").await;
                }
                self.publish_rust_analyzer_status(&mut state).await;
            }
        }

        rust_glancer::deferred_indexing_finished(&self.lsp_client, root).await;
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
            && state.deferred_indexing.contains_key(root)
            && self.capabilities.work_done_progress
        {
            let progress = state
                .deferred_indexing
                .get(root)
                .and_then(|deferred| deferred.progress);
            state
                .workspace_progress
                .begin_deferred(&self.lsp_client, root)
                .await;
            if let Some(progress) = progress {
                state.workspace_progress.report(root, progress).await;
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
    workspace_progress: work_done_progress::WorkspaceProgressState,
    last_rust_analyzer_status: Option<rust_analyzer::StatusSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeferredIndexingState {
    generation: u64,
    progress: Option<IndexingProgress>,
}

impl ClientStatusState {
    /// Collapse per-workspace state into rust-analyzer's server-wide health vocabulary.
    fn rust_analyzer_status(&self) -> rust_analyzer::StatusSnapshot {
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
        let health = if failures.is_empty() {
            rust_analyzer::Health::Ok
        } else if failures.len() == self.workspaces.len() {
            rust_analyzer::Health::Error
        } else {
            rust_analyzer::Health::Warning
        };
        let message = failures.first().map(|(root, error)| {
            let additional = failures.len().saturating_sub(1);
            let suffix = if additional == 0 {
                String::new()
            } else {
                format!(" (+{additional} more)")
            };
            format!("Workspace `{}` failed: {error}{suffix}", root.display())
        });
        let foreground_work = self
            .workspaces
            .values()
            .any(|lifecycle| matches!(lifecycle, WorkspaceLifecycle::Indexing));

        rust_analyzer::StatusSnapshot {
            health,
            quiescent: !foreground_work && self.deferred_indexing.is_empty(),
            message,
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
mod tests {
    use futures::{SinkExt as _, StreamExt as _};
    use rg_lsp_proto::IndexingStage;
    use tower::{Service as _, ServiceExt as _};
    use tower_lsp_server::{
        LanguageServer, LspService,
        jsonrpc::{Request, Response, Result},
        ls_types::{
            ClientCapabilities, InitializeParams, InitializeResult, WindowClientCapabilities,
        },
    };

    use super::*;

    #[test]
    fn reads_negotiated_status_capabilities() {
        let capabilities = ClientCapabilities {
            experimental: Some(serde_json::json!({
                "serverStatusNotification": true,
            })),
            window: Some(WindowClientCapabilities {
                work_done_progress: Some(true),
                ..WindowClientCapabilities::default()
            }),
            ..ClientCapabilities::default()
        };

        assert_eq!(
            ClientStatusCapabilities::from_lsp_client_capabilities(&capabilities),
            ClientStatusCapabilities {
                work_done_progress: true,
                rust_analyzer_server_status: true,
            }
        );
    }

    #[test]
    fn rust_analyzer_status_separates_health_from_background_work() {
        let mut state = ClientStatusState::default();
        let ready = PathBuf::from("/workspace/ready");
        let failed = PathBuf::from("/workspace/failed");
        state
            .workspaces
            .insert(ready.clone(), WorkspaceLifecycle::Indexing);

        assert_eq!(
            state.rust_analyzer_status(),
            rust_analyzer::StatusSnapshot {
                health: rust_analyzer::Health::Ok,
                quiescent: false,
                message: None,
            }
        );

        state
            .workspaces
            .insert(ready.clone(), WorkspaceLifecycle::Ready);
        state.workspaces.insert(
            failed.clone(),
            WorkspaceLifecycle::Failed(Arc::from("engine exited")),
        );
        let partially_failed = state.rust_analyzer_status();
        assert_eq!(partially_failed.health, rust_analyzer::Health::Warning);
        assert!(partially_failed.quiescent);
        assert_eq!(
            partially_failed.message.as_deref(),
            Some("Workspace `/workspace/failed` failed: engine exited")
        );

        state.deferred_indexing.insert(
            ready,
            DeferredIndexingState {
                generation: 1,
                progress: None,
            },
        );
        assert!(!state.rust_analyzer_status().quiescent);

        state.workspaces.remove(&failed);
        assert_eq!(
            state.rust_analyzer_status().health,
            rust_analyzer::Health::Ok
        );
    }

    #[test]
    fn only_failed_workspace_makes_the_server_unhealthy() {
        let mut state = ClientStatusState::default();
        state.workspaces.insert(
            PathBuf::from("/workspace/failed"),
            WorkspaceLifecycle::Failed(Arc::from("metadata failed")),
        );

        assert_eq!(
            state.rust_analyzer_status().health,
            rust_analyzer::Health::Error
        );
    }

    #[tokio::test]
    async fn indexing_lifecycle_publishes_negotiated_client_protocol() {
        let capabilities = ClientStatusCapabilities {
            work_done_progress: true,
            rust_analyzer_server_status: true,
        };
        let (mut service, mut socket) = LspService::new(move |client| TestBackend {
            client_status: ClientStatusPublisher::new(client, capabilities),
        });
        let initialize = Request::build("initialize")
            .params(serde_json::json!({"capabilities": {}}))
            .id(1)
            .finish();
        service
            .ready()
            .await
            .expect("test LSP service should become ready")
            .call(initialize)
            .await
            .expect("initialize request should reach the test service")
            .expect("initialize request should return a response");

        let client_status = service.inner().client_status.clone();
        let root = Path::new("/workspace/project_a");
        let publish = async {
            client_status.workspace_indexing(root).await;
            client_status.workspace_ready(root).await;
        };
        let observe = async {
            let create = socket
                .next()
                .await
                .expect("indexing should create a work-done token");
            assert_eq!(create.method(), "window/workDoneProgress/create");
            let token = create
                .params()
                .and_then(|params| params.get("token"))
                .cloned()
                .expect("work-done token creation should name the token");
            let create_id = create
                .id()
                .cloned()
                .expect("work-done token creation should be a request");
            socket
                .send(Response::from_ok(create_id, serde_json::Value::Null))
                .await
                .expect("test client should acknowledge the work-done token");

            let begin = socket
                .next()
                .await
                .expect("indexing should begin work-done progress");
            assert_eq!(begin.method(), "$/progress");
            assert_eq!(
                begin.params(),
                Some(&serde_json::json!({
                    "token": token.clone(),
                    "value": {
                        "kind": "begin",
                        "title": "Indexing project_a",
                        "cancellable": false,
                    },
                }))
            );

            let working = socket
                .next()
                .await
                .expect("indexing should make the server non-quiescent");
            assert_eq!(working.method(), "experimental/serverStatus");
            assert_eq!(
                working.params(),
                Some(&serde_json::json!({
                    "health": "ok",
                    "quiescent": false,
                }))
            );

            let end = socket
                .next()
                .await
                .expect("ready workspace should finish work-done progress");
            assert_eq!(end.method(), "$/progress");
            assert_eq!(
                end.params(),
                Some(&serde_json::json!({
                    "token": token,
                    "value": {
                        "kind": "end",
                        "message": "Finished",
                    },
                }))
            );

            let ready = socket
                .next()
                .await
                .expect("ready workspace should make the server quiescent");
            assert_eq!(ready.method(), "experimental/serverStatus");
            assert_eq!(
                ready.params(),
                Some(&serde_json::json!({
                    "health": "ok",
                    "quiescent": true,
                }))
            );
        };

        tokio::join!(publish, observe);
    }

    #[tokio::test]
    async fn deferred_progress_replaces_foreground_progress_after_queryable_boundary() {
        let capabilities = ClientStatusCapabilities {
            work_done_progress: true,
            rust_analyzer_server_status: false,
        };
        let (mut service, mut socket) = LspService::new(move |client| TestBackend {
            client_status: ClientStatusPublisher::new(client, capabilities),
        });
        let initialize = Request::build("initialize")
            .params(serde_json::json!({"capabilities": {}}))
            .id(1)
            .finish();
        service
            .ready()
            .await
            .expect("test LSP service should become ready")
            .call(initialize)
            .await
            .expect("initialize request should reach the test service")
            .expect("initialize request should return a response");

        let client_status = service.inner().client_status.clone();
        let root = Path::new("/workspace/project_a");
        let first_progress = IndexingProgress {
            stage: IndexingStage::LoweringBodies,
            completed_packages: 4,
            total_packages: 12,
        };
        let second_progress = IndexingProgress {
            stage: IndexingStage::ResolvingBodies,
            completed_packages: 7,
            total_packages: 12,
        };
        let publish = async {
            client_status.workspace_indexing(root).await;
            client_status.deferred_indexing_started(root, 7).await;
            client_status
                .deferred_indexing_progress(root, 7, first_progress)
                .await;
            client_status.workspace_ready(root).await;
            client_status
                .deferred_indexing_progress(root, 7, second_progress)
                .await;
            client_status.deferred_indexing_finished(root, 7).await;
        };
        let observe = async {
            let foreground_create = socket
                .next()
                .await
                .expect("foreground indexing should create a progress token");
            let foreground_token = foreground_create
                .params()
                .and_then(|params| params.get("token"))
                .cloned()
                .expect("foreground progress should name its token");
            socket
                .send(Response::from_ok(
                    foreground_create
                        .id()
                        .cloned()
                        .expect("progress creation should be a request"),
                    serde_json::Value::Null,
                ))
                .await
                .expect("test client should acknowledge foreground progress");

            let foreground_begin = socket
                .next()
                .await
                .expect("foreground progress should begin");
            assert_eq!(foreground_begin.method(), "$/progress");

            let private_start = socket
                .next()
                .await
                .expect("private deferred lifecycle should remain additive");
            assert_eq!(
                private_start.method(),
                "rust-glancer/deferredIndexingStarted"
            );

            let foreground_end = socket
                .next()
                .await
                .expect("queryable workspace should end foreground progress");
            assert_eq!(
                foreground_end.params(),
                Some(&serde_json::json!({
                    "token": foreground_token,
                    "value": {"kind": "end", "message": "Finished"},
                }))
            );

            let deferred_create = socket
                .next()
                .await
                .expect("pending deferred work should create its own token");
            let deferred_token = deferred_create
                .params()
                .and_then(|params| params.get("token"))
                .cloned()
                .expect("deferred progress should name its token");
            socket
                .send(Response::from_ok(
                    deferred_create
                        .id()
                        .cloned()
                        .expect("progress creation should be a request"),
                    serde_json::Value::Null,
                ))
                .await
                .expect("test client should acknowledge deferred progress");

            let deferred_begin = socket.next().await.expect("deferred progress should begin");
            assert_eq!(
                deferred_begin.params(),
                Some(&serde_json::json!({
                    "token": deferred_token.clone(),
                    "value": {
                        "kind": "begin",
                        "title": "project_a ready · background",
                        "cancellable": false,
                    },
                }))
            );

            let replayed = socket
                .next()
                .await
                .expect("latest deferred progress should be replayed after foreground work");
            assert_eq!(
                replayed.params(),
                Some(&serde_json::json!({
                    "token": deferred_token.clone(),
                    "value": {
                        "kind": "report",
                        "message": "Lowering · 4/12",
                    },
                }))
            );

            let advanced = socket
                .next()
                .await
                .expect("new deferred progress should update the same token");
            assert_eq!(
                advanced.params(),
                Some(&serde_json::json!({
                    "token": deferred_token.clone(),
                    "value": {
                        "kind": "report",
                        "message": "Resolving · 7/12",
                    },
                }))
            );

            let deferred_end = socket
                .next()
                .await
                .expect("deferred finish should close its progress token");
            assert_eq!(
                deferred_end.params(),
                Some(&serde_json::json!({
                    "token": deferred_token,
                    "value": {"kind": "end", "message": "Finished"},
                }))
            );

            let private_finish = socket
                .next()
                .await
                .expect("private deferred finish should remain additive");
            assert_eq!(
                private_finish.method(),
                "rust-glancer/deferredIndexingFinished"
            );
        };

        tokio::join!(publish, observe);
    }

    #[tokio::test]
    async fn stale_deferred_generation_cannot_replace_or_finish_current_progress() {
        let (service, _socket) = LspService::new(|client| TestBackend {
            client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
        });
        let client_status = service.inner().client_status.clone();
        let root = Path::new("/workspace/project_a");
        let progress = IndexingProgress {
            stage: IndexingStage::ResolvingBodies,
            completed_packages: 2,
            total_packages: 8,
        };

        client_status.deferred_indexing_started(root, 9).await;
        client_status
            .deferred_indexing_progress(root, 8, progress)
            .await;
        client_status.deferred_indexing_finished(root, 8).await;

        let state = client_status.state.lock().await;
        assert_eq!(
            state.deferred_indexing.get(root),
            Some(&DeferredIndexingState {
                generation: 9,
                progress: None,
            })
        );
    }

    #[tokio::test]
    async fn rust_glancer_notifications_remain_additive_without_negotiation() {
        let (mut service, mut socket) = LspService::new(|client| TestBackend {
            client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
        });
        let initialize = Request::build("initialize")
            .params(serde_json::json!({"capabilities": {}}))
            .id(1)
            .finish();
        service
            .ready()
            .await
            .expect("test LSP service should become ready")
            .call(initialize)
            .await
            .expect("initialize request should reach the test service")
            .expect("initialize request should return a response");

        let client_status = service.inner().client_status.clone();
        let root = Path::new("/workspace/project_a");

        client_status
            .active_workspace_changed(Some(ActiveWorkspaceStatus {
                root: root.to_path_buf(),
                state: ActiveWorkspaceState::Ready,
                message: None,
            }))
            .await;
        let active = socket
            .next()
            .await
            .expect("active workspace should use the Rust Glancer protocol");
        assert_eq!(active.method(), "rust-glancer/activeWorkspaceChanged");
        assert_eq!(
            active.params(),
            Some(&serde_json::json!({
                "root": "/workspace/project_a",
                "state": "ready",
            }))
        );

        client_status.deferred_indexing_started(root, 1).await;
        let deferred_started = socket
            .next()
            .await
            .expect("deferred indexing should publish its private start event");
        assert_eq!(
            deferred_started.method(),
            "rust-glancer/deferredIndexingStarted"
        );
        assert_eq!(
            deferred_started.params(),
            Some(&serde_json::json!({"root": "/workspace/project_a"}))
        );

        client_status.deferred_indexing_finished(root, 1).await;
        let deferred_finished = socket
            .next()
            .await
            .expect("deferred indexing should publish its private finish event");
        assert_eq!(
            deferred_finished.method(),
            "rust-glancer/deferredIndexingFinished"
        );
        assert_eq!(
            deferred_finished.params(),
            Some(&serde_json::json!({"root": "/workspace/project_a"}))
        );
    }

    #[tokio::test]
    async fn terminal_engine_failure_ignores_late_project_status() {
        let (service, _socket) = LspService::new(|client| TestBackend {
            client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
        });
        let client_status = service.inner().client_status.clone();
        let root = Path::new("/workspace/project_a");

        client_status
            .workspace_unavailable(root, "engine exited")
            .await;
        client_status.workspace_indexing(root).await;
        client_status.workspace_ready(root).await;
        client_status.deferred_indexing_started(root, 1).await;

        let state = client_status.state.lock().await;
        assert_eq!(
            state.workspaces.get(root),
            Some(&WorkspaceLifecycle::Unavailable(Arc::from("engine exited")))
        );
        assert!(state.deferred_indexing.is_empty());
    }

    #[derive(Debug)]
    struct TestBackend {
        client_status: ClientStatusPublisher,
    }

    impl LanguageServer for TestBackend {
        async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
            Ok(InitializeResult::default())
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }
}
