//! Multi-engine lifecycle and routing owned by the LSP server.
//!
//! The registry is where a raw editor path is associated with one engine process. Opening a
//! document freezes that engine and its project-source identity in `OpenDocumentRoute`; later
//! requests reuse the session route instead of rediscovering ownership from the filesystem.
//! Native filesystem batches take the other path: they are grouped by already-known engine roots,
//! captured once, and forwarded as saved-project mutations.

use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use rg_lsp_proto::{CapturedSourceInput, EngineConfig, SavedProjectChanges};
use rg_std::UniqueVec;
use tokio::sync::Mutex;
use tower_lsp_server::{Client as LspClient, ls_types::MessageType};

use crate::{
    client_status::{ClientStatusCapabilities, ClientStatusPublisher},
    config::ServerConfig,
    engine_client::{EngineClient, EngineProjectStatus, EngineProjectUpdate},
    engine_process::{EngineProcess, EngineProcessExit, EngineProcessExitMonitor},
    ingress::EditorStateHandle,
};

mod document_owner;
pub(crate) mod routing;
mod slot;
mod state;

use self::{
    document_owner::DocumentOwner,
    routing::{EngineId, normalize_path},
    slot::{EngineEntry, EngineSlot},
    state::{EngineRegistryInner, ReservedEngineRoute, ReservedEngineStart},
};

/// Routes LSP requests to the engine process that owns the requested file.
///
/// The server process is the only place that knows about multiple engines. Routing owns path/root
/// decisions, while the registry owns engine lifecycle slots and RPC clients.
#[derive(Clone, Debug)]
pub(crate) struct EngineRegistry {
    lsp_client: LspClient,
    client_status: ClientStatusPublisher,
    editor: EditorStateHandle,
    inner: Arc<Mutex<EngineRegistryInner>>,
}

/// Engine ownership plus the project-source identity selected while routing one open document.
///
/// The editor URI can outlive its filesystem spelling: a rename or removal after `didOpen` must
/// not force later analysis to rediscover which source the open session originally addressed.
#[derive(Clone, Debug)]
pub(crate) struct OpenDocumentRoute {
    engine_client: EngineClient,
    source_path: PathBuf,
}

impl OpenDocumentRoute {
    pub(crate) fn new(engine_client: EngineClient, source_path: PathBuf) -> Self {
        Self {
            engine_client,
            source_path,
        }
    }

    pub(crate) fn engine_client(&self) -> &EngineClient {
        &self.engine_client
    }

    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Foreground project updates held while one native watcher burst settles and rebuilds.
///
/// The watcher is scoped to an editor workspace folder, which may contain several already-started
/// Cargo engines. Keeping their updates together lets every affected status change happen before
/// the watcher waits for quiet. An update that finds no forwarded path is dropped as cancelled, so
/// filtering a watcher event cannot accidentally clear an older failed status.
#[must_use = "external project changes must be forwarded after the watcher settles"]
#[derive(Debug)]
pub(crate) struct ExternalProjectChanges {
    updates: BTreeMap<EngineId, EngineProjectUpdate>,
}

impl EngineRegistry {
    /// Creates a registry that can spawn engines and forward their notifications to the LSP client.
    pub(crate) fn new(
        lsp_client: LspClient,
        workspace_folders: Vec<PathBuf>,
        config: ServerConfig,
        editor: EditorStateHandle,
        client_status_capabilities: ClientStatusCapabilities,
    ) -> Self {
        let client_status =
            ClientStatusPublisher::new(lsp_client.clone(), client_status_capabilities);
        Self {
            lsp_client,
            client_status,
            editor,
            inner: Arc::new(Mutex::new(EngineRegistryInner::new(
                workspace_folders,
                config,
            ))),
        }
    }

    /// Capture existing Rust source once; preserve graph- and deletion-shaped inputs as paths.
    async fn capture_external_project_changes(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<SavedProjectChanges> {
        let mut captured_sources = Vec::new();
        let mut fs_path_changes = Vec::new();

        for path in paths {
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
                fs_path_changes.push(path);
                continue;
            }

            match tokio::fs::read_to_string(&path).await {
                Ok(text) => captured_sources.push(CapturedSourceInput::new(path, text)),
                Err(error) if error.kind() == ErrorKind::NotFound => fs_path_changes.push(path),
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "read external Rust source `{}` before submission: {error}",
                        path.display()
                    ));
                }
            }
        }

        Ok(SavedProjectChanges::new(captured_sources, fs_path_changes))
    }

    /// Returns every ready engine client for lifecycle fan-out such as shutdown.
    pub(crate) async fn engine_clients(&self) -> Vec<EngineClient> {
        let inner = self.inner.lock().await;
        inner
            .engines
            .iter()
            .filter_map(|slot| {
                slot.ready()
                    .map(|engine| engine.process.engine_client().clone())
            })
            .collect()
    }

    /// Returns the currently active ready engine, if one has been selected.
    pub(crate) async fn active_engine(&self) -> anyhow::Result<Option<EngineClient>> {
        let inner = self.inner.lock().await;
        let Some(id) = inner.routing.active_id() else {
            return Ok(None);
        };
        match inner.engine(id) {
            Some(EngineSlot::Ready(engine)) => Ok(Some(engine.process.engine_client().clone())),
            Some(EngineSlot::Starting { .. }) | None => Ok(None),
            Some(EngineSlot::Failed { root, error }) => Err(anyhow::anyhow!(
                "rust-glancer engine for `{}` is unavailable: {error}",
                root.display()
            )),
        }
    }

    /// Prevents expected child exits during LSP shutdown from becoming user-facing failures.
    pub(crate) async fn begin_shutdown(&self) {
        self.inner.lock().await.begin_shutdown();
    }

    /// Routes a newly opened document and records exact file ownership until `didClose`.
    pub(crate) async fn open_document(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<OpenDocumentRoute>> {
        let path = normalize_path(path);
        let owner = {
            let mut inner = self.inner.lock().await;
            DocumentOwner::new(&mut inner, &path)?
        };
        let Some(owner) = owner else {
            return Ok(None);
        };

        let id = owner.id();
        match self.engine_for_document_owner(owner).await {
            Ok(Some(engine_client)) => Ok(Some(OpenDocumentRoute::new(engine_client, path))),
            Ok(None) => {
                self.remove_open_file(path.as_path(), id).await;
                Ok(None)
            }
            Err(error) => {
                self.remove_open_file(path.as_path(), id).await;
                Err(error)
            }
        }
    }

    /// Forgets the route remembered while a document was open.
    pub(crate) async fn close_document(&self, path: &Path) {
        self.inner.lock().await.remove_open_file(path, None);
    }

    /// Mark every ready Cargo engine below one watched editor folder as updating.
    ///
    /// This happens on the first relevant filesystem event, before the watcher waits for a quiet
    /// period. It only touches already-known engines: a native event must not start a Cargo engine
    /// for a workspace the editor has never routed to.
    pub(crate) async fn begin_external_project_changes(
        &self,
        workspace_root: &Path,
    ) -> ExternalProjectChanges {
        let inner = self.inner.lock().await;
        let updates = inner
            .routing
            .engine_ids_for_workspace(workspace_root)
            .filter_map(|id| {
                let EngineSlot::Ready(engine) = inner.engine(id)? else {
                    return None;
                };
                Some((id, engine.process.engine_client().begin_project_update()))
            })
            .collect();

        ExternalProjectChanges { updates }
    }

    /// Finish one settled watcher burst against the ready engines that own its paths.
    ///
    /// `changes` carries the project updates acquired before the settle delay. Each routed RPC
    /// completes its matching update as success or failure. Updates for filtered paths fall
    /// out of scope as cancellations and therefore preserve any older failure.
    pub(crate) async fn finish_external_project_changes(
        &self,
        paths: Vec<PathBuf>,
        mut changes: ExternalProjectChanges,
    ) {
        // One editor workspace folder can contain several Cargo roots. Route and deduplicate the
        // settled paths under one registry snapshot; unknown or non-ready engines stay untouched.
        let paths_by_engine = {
            let inner = self.inner.lock().await;
            let mut grouped = BTreeMap::<EngineId, (EngineClient, UniqueVec<PathBuf>)>::new();

            for path in paths {
                let path = normalize_path(path);
                let Some(id) = inner.routing.engine_id_for_known_root_path(&path) else {
                    tracing::trace!(
                        path = %path.display(),
                        "external project path skipped without known engine root"
                    );
                    continue;
                };
                let Some(EngineSlot::Ready(engine)) = inner.engine(id) else {
                    tracing::trace!(
                        path = %path.display(),
                        engine_id = id.index(),
                        "external project path skipped because engine is not ready"
                    );
                    continue;
                };

                let (_, paths) = grouped
                    .entry(id)
                    .or_insert_with(|| (engine.process.engine_client().clone(), UniqueVec::new()));
                paths.push(path);
            }

            grouped
                .into_iter()
                .map(|(id, (engine_client, paths))| (id, engine_client, paths.into_vec()))
                .collect::<Vec<_>>()
        };

        // The watcher already published updating status before waiting for quiet. Transfer each
        // early update to its RPC so cancellation cannot publish readiness while an accepted
        // engine mutation is still queued or running.
        for (id, engine_client, paths) in paths_by_engine {
            let update = changes
                .updates
                .remove(&id)
                .unwrap_or_else(|| engine_client.begin_project_update());

            // Existing Rust sources become immutable request values here. The engine rebuilds from
            // these exact bytes, then independently validates them against disk immediately before
            // publication. Missing sources remain path-shaped because deletion and discovery need
            // the project layer to interpret the filesystem transition.
            let request = match Self::capture_external_project_changes(paths).await {
                Ok(request) => request,
                Err(error) => {
                    update.fail(&error);
                    tracing::warn!(
                        engine_id = id.index(),
                        error = %format!("{error:#}"),
                        "failed to capture external project changes"
                    );
                    continue;
                }
            };
            let result = engine_client
                .call_with_project_update(
                    update,
                    "external_project_changes",
                    |engine_client, request_context| async move {
                        engine_client
                            .external_project_changes(request_context, request)
                            .await
                    },
                )
                .await;
            if let Err(error) = result {
                tracing::warn!(
                    engine_id = id.index(),
                    error = %format!("{error:#}"),
                    "failed to apply external project changes"
                );
            }
        }
        // Remaining guards belong to paths that were filtered out or lost their ready engine. Their
        // `Drop` implementation cancels the update without treating it as a successful retry.
    }

    async fn engine_for_document_owner(
        &self,
        owner: DocumentOwner,
    ) -> anyhow::Result<Option<EngineClient>> {
        let id = owner.id();
        tracing::trace!(
            engine_id = id.index(),
            source = ?owner.source(),
            "resolved document owner"
        );

        let route = owner.into_route();
        self.activate_workspace(id).await;

        let engine_client = match route {
            ReservedEngineRoute::Existing(id) => self.engine_for_existing_id(id).await?,
            ReservedEngineRoute::Spawn(start) => Some(self.start_reserved_engine(start).await?),
        };
        Ok(engine_client)
    }

    async fn remove_open_file(&self, path: &Path, id: EngineId) {
        let mut inner = self.inner.lock().await;
        inner.remove_open_file(path, Some(id));
    }

    async fn activate_workspace(&self, id: EngineId) {
        let status = {
            let mut inner = self.inner.lock().await;
            inner.set_active_id(id);
            inner.workspace_status_update()
        };

        self.client_status.active_workspace_changed(status).await;
    }

    /// Materializes a reserved engine id into a ready engine process.
    async fn start_reserved_engine(
        &self,
        start: ReservedEngineStart,
    ) -> anyhow::Result<EngineClient> {
        self.client_status.workspace_indexing(&start.root).await;
        let spawned = self.spawn_engine(start.root.clone(), start.config).await;
        let (engine, exit_monitor) = match spawned {
            Ok(engine) => engine,
            Err(error) => {
                self.mark_failed(start.id, start.root, error.to_string())
                    .await;
                return Err(error);
            }
        };
        let engine_client = engine.engine_client().clone();

        // Every engine follows the same lifecycle: protocol initialize first, then the
        // post-initialize notification before it can be observed as ready by request routing.
        if let Err(error) = engine_client
            .call_unconditional("initialized", |engine_client, request_context| async move {
                engine_client.initialized(request_context).await
            })
            .await
        {
            engine_client
                .notify("shutdown", |engine_client, request_context| async move {
                    engine_client.shutdown(request_context).await
                })
                .await;
            self.mark_failed(start.id, start.root, error.to_string())
                .await;
            return Err(error);
        }

        self.mark_ready(start.id, start.root.clone(), engine).await;

        let inner = Arc::downgrade(&self.inner);
        let lsp_client = self.lsp_client.clone();
        let client_status = self.client_status.clone();
        let id = start.id;
        let root = start.root;
        // This is deliberately supervision, not recovery. An engine panic is a bug that should stay
        // visible enough to report and fix, while automatic replacement would risk hiding the real
        // problem or confusing server-side routing state.
        // Once startup succeeds, this task is the whole supervision layer for the child process:
        // it waits for one terminal event and marks the ready engine failed.
        tokio::spawn(async move {
            let Some(exit) = exit_monitor.wait().await else {
                return;
            };

            Self::mark_exited(inner, lsp_client, client_status, id, root, exit).await;
        });

        Ok(engine_client)
    }

    /// Returns a ready engine RPC client for an existing id, waiting if startup is still in progress.
    async fn engine_for_existing_id(&self, id: EngineId) -> anyhow::Result<Option<EngineClient>> {
        loop {
            let wait = {
                let inner = self.inner.lock().await;
                match inner.engine(id) {
                    Some(EngineSlot::Ready(engine)) => {
                        let engine_client = engine.process.engine_client().clone();
                        return Ok(Some(engine_client));
                    }
                    // Create the waiter under the publication lock. `notify_waiters` keeps no
                    // permit, so creating it after unlock could miss the startup transition.
                    Some(EngineSlot::Starting { notify, .. }) => {
                        Some(notify.clone().notified_owned())
                    }
                    Some(EngineSlot::Failed { root, error }) => {
                        return Err(anyhow::anyhow!(
                            "rust-glancer engine for `{}` is unavailable: {error}",
                            root.display()
                        ));
                    }
                    None => return Ok(None),
                }
            };

            // Existing ids can point at a reserved-but-not-ready slot. Wait outside the registry
            // lock so the task that is starting the engine can mark the slot ready or failed.
            wait.expect("starting engine should provide notification")
                .await;
        }
    }

    /// Replaces a starting slot with a ready process and wakes waiters.
    async fn mark_ready(&self, id: EngineId, root: PathBuf, process: EngineProcess) {
        let project_status = process.engine_client().project_status_changes();
        let (notify, status) = {
            let mut inner = self.inner.lock().await;
            let notify = inner
                .engine(id)
                .and_then(EngineSlot::notify)
                .expect("reserved engine slot should be starting");
            inner.engines[id.index()] = EngineSlot::Ready(EngineEntry { process });
            let status = inner.workspace_status_update();
            (notify, status)
        };
        notify.notify_waiters();
        self.client_status.workspace_ready(&root).await;
        self.client_status.active_workspace_changed(status).await;
        self.spawn_project_status_monitor(id, root, project_status);
    }

    /// Republish active-workspace status when a ready process starts or finishes foreground work.
    fn spawn_project_status_monitor(
        &self,
        id: EngineId,
        root: PathBuf,
        mut project_status: tokio::sync::watch::Receiver<EngineProjectStatus>,
    ) {
        let inner = Arc::downgrade(&self.inner);
        let client_status = self.client_status.clone();
        tokio::spawn(async move {
            while project_status.changed().await.is_ok() {
                let project_status = project_status.borrow_and_update().clone();
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                let status = {
                    let mut inner = inner.lock().await;
                    if !matches!(inner.engine(id), Some(EngineSlot::Ready(_))) {
                        return;
                    }
                    inner.workspace_status_update()
                };
                match project_status {
                    EngineProjectStatus::Ready => client_status.workspace_ready(&root).await,
                    EngineProjectStatus::Updating => {
                        client_status.workspace_indexing(&root).await;
                    }
                    EngineProjectStatus::Failed(error) => {
                        client_status.workspace_failed(&root, error).await;
                    }
                }
                client_status.active_workspace_changed(status).await;
            }
        });
    }

    /// Replaces a starting slot with a failure and wakes waiters.
    async fn mark_failed(&self, id: EngineId, root: PathBuf, error: String) {
        let status_root = root.clone();
        let status_error = Arc::<str>::from(error);
        let (notify, status) = {
            let mut inner = self.inner.lock().await;
            let notify = inner.engine(id).and_then(EngineSlot::notify);
            inner.engines[id.index()] = EngineSlot::Failed {
                root,
                error: Arc::clone(&status_error),
            };
            let status = inner.workspace_status_update();
            (notify, status)
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
        self.client_status
            .workspace_unavailable(&status_root, status_error)
            .await;
        self.client_status.active_workspace_changed(status).await;
    }

    async fn mark_exited(
        inner: Weak<Mutex<EngineRegistryInner>>,
        lsp_client: LspClient,
        client_status: ClientStatusPublisher,
        id: EngineId,
        root: PathBuf,
        exit: EngineProcessExit,
    ) {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let error = exit.failure_message();
        let status = {
            let mut inner = inner.lock().await;
            if inner.shutting_down() {
                return;
            }

            match inner.engine(id) {
                Some(EngineSlot::Ready(_)) => {
                    inner.engines[id.index()] = EngineSlot::Failed {
                        root: root.clone(),
                        error: Arc::from(error.as_str()),
                    };
                    inner.workspace_status_update()
                }
                _ => {
                    return;
                }
            }
        };

        tracing::error!(
            engine_id = id.index(),
            root = %root.display(),
            error = %error,
            "rust-glancer engine became unavailable"
        );
        lsp_client
            .log_message(MessageType::ERROR, format!("Rust Glancer {error}"))
            .await;
        client_status
            .workspace_unavailable(&root, Arc::<str>::from(error))
            .await;
        client_status.active_workspace_changed(status).await;
    }

    /// Spawns the engine subprocess and sends its protocol initialize request.
    async fn spawn_engine(
        &self,
        root: PathBuf,
        config: EngineConfig,
    ) -> anyhow::Result<(EngineProcess, EngineProcessExitMonitor)> {
        let (engine, exit_monitor) = EngineProcess::spawn(
            self.lsp_client.clone(),
            self.editor.clone(),
            self.client_status.clone(),
            &root,
            Self::engine_id(&root),
        )
        .await?;
        let engine_client = engine.engine_client().clone();
        let initialize_root = root.clone();
        engine_client
            .call_unconditional(
                "initialize",
                move |engine_client, request_context| async move {
                    engine_client
                        .initialize(request_context, initialize_root, config)
                        .await
                },
            )
            .await?;

        tracing::info!(root = %root.display(), "started rust-glancer engine");
        Ok((engine, exit_monitor))
    }

    fn engine_id(root: &Path) -> String {
        root.file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rg_lsp_proto::EngineConfig;
    use test_fixture::{CrateFixture, fixture_crate};
    use tower_lsp_server::{
        ClientSocket, LanguageServer, LspService,
        jsonrpc::Result,
        ls_types::{InitializeParams, InitializeResult},
    };

    use crate::client_status::ActiveWorkspaceState;

    use super::document_owner::DocumentOwnerSource;
    use super::*;

    const WORKSPACE_FIXTURE: &str = r#"
//- /workspace/Cargo.toml
[workspace]
members = ["project_a"]
resolver = "3"

//- /workspace/project_a/Cargo.toml
[package]
name = "project_a"
version = "0.1.0"
edition = "2024"

//- /workspace/project_a/src/lib.rs
pub struct ProjectA;
"#;

    #[tokio::test]
    async fn watcher_captures_existing_rust_text_and_preserves_graph_or_deletion_paths() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let source = fixture.path("workspace/project_a/src/lib.rs");
        let manifest = fixture.path("workspace/project_a/Cargo.toml");
        let deleted_source = fixture.path("workspace/project_a/src/deleted.rs");
        std::fs::write(&source, "pub struct Captured;\n")
            .expect("watcher fixture source should be writable");

        let changes = EngineRegistry::capture_external_project_changes(vec![
            source.clone(),
            manifest.clone(),
            deleted_source.clone(),
        ])
        .await
        .expect("ordinary watcher inputs should be captured");
        std::fs::write(&source, "pub struct Later;\n")
            .expect("disk should be able to advance after capture");

        assert_eq!(changes.captured_sources().len(), 1);
        assert_eq!(changes.captured_sources()[0].path(), source);
        assert_eq!(
            changes.captured_sources()[0].text(),
            "pub struct Captured;\n",
            "the RPC must retain the value from watcher capture"
        );
        assert_eq!(changes.fs_paths(), [manifest, deleted_source]);
    }

    #[tokio::test]
    async fn open_document_records_owner_before_engine_startup_completes() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("workspace/project_a/src/lib.rs");

        let owner = {
            let mut inner = registry.inner.lock().await;
            DocumentOwner::new(&mut inner, &document)
                .expect("open document should route through Cargo workspace")
                .expect("workspace document should have an owner")
        };
        let cached_owner = {
            let inner = registry.inner.lock().await;
            inner.open_file_owner(&document)
        };

        assert!(matches!(
            owner.source(),
            DocumentOwnerSource::CargoWorkspace
        ));
        assert_eq!(cached_owner, Some(owner.id()));
        assert!(matches!(
            registry.inner.lock().await.engine(owner.id()),
            Some(EngineSlot::Starting { .. })
        ));
    }

    #[tokio::test]
    async fn outside_workspace_document_does_not_invoke_cargo_locate_project() {
        let fixture = fixture_crate(&format!(
            "{WORKSPACE_FIXTURE}\n{}",
            r#"
//- /external/Cargo.toml
this is not a valid Cargo manifest

//- /external/src/lib.rs
pub struct External;
"#,
        ));
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("external/src/lib.rs");

        let owner = {
            let mut inner = registry.inner.lock().await;
            DocumentOwner::new(&mut inner, &document)
                .expect("outside workspace document should not run cargo locate-project")
        };

        assert!(owner.is_none());
    }

    #[tokio::test]
    async fn active_workspace_status_tracks_reserved_engine_lifecycle() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("workspace/project_a/src/lib.rs");
        let workspace_root = normalize_path(fixture.path("workspace"));

        let owner = {
            let mut inner = registry.inner.lock().await;
            DocumentOwner::new(&mut inner, &document)
                .expect("open document should route through Cargo workspace")
                .expect("workspace document should have an owner")
        };
        let id = owner.id();

        let indexing = {
            let mut inner = registry.inner.lock().await;
            inner.set_active_id(id);
            inner.workspace_status_update()
        }
        .expect("new active workspace status should be published");
        assert_eq!(indexing.root, workspace_root);
        assert_eq!(indexing.state, ActiveWorkspaceState::Indexing);
        assert_eq!(indexing.message, None);

        let duplicate = {
            let mut inner = registry.inner.lock().await;
            inner.set_active_id(id);
            inner.workspace_status_update()
        };
        assert_eq!(duplicate, None);

        let failed = {
            let mut inner = registry.inner.lock().await;
            inner.engines[id.index()] = EngineSlot::Failed {
                root: workspace_root.clone(),
                error: Arc::from("startup failed"),
            };
            inner.workspace_status_update()
        }
        .expect("changed active workspace status should be published");
        assert_eq!(failed.root, workspace_root);
        assert_eq!(failed.state, ActiveWorkspaceState::Failed);
        assert_eq!(failed.message.as_deref(), Some("startup failed"));
    }

    #[tokio::test]
    async fn active_engine_reports_failed_slot() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("workspace/project_a/src/lib.rs");
        let workspace_root = normalize_path(fixture.path("workspace"));

        let id = {
            let mut inner = registry.inner.lock().await;
            let owner = DocumentOwner::new(&mut inner, &document)
                .expect("open document should route through Cargo workspace")
                .expect("workspace document should have an owner");
            inner.set_active_id(owner.id());
            inner.engines[owner.id().index()] = EngineSlot::Failed {
                root: workspace_root.clone(),
                error: Arc::from("engine process exited unexpectedly: exit status: 101"),
            };
            owner.id()
        };

        let error = registry
            .active_engine()
            .await
            .expect_err("failed active engine should be user-visible");

        assert_eq!(id.index(), 0);
        assert_eq!(
            error.to_string(),
            format!(
                "rust-glancer engine for `{}` is unavailable: engine process exited unexpectedly: exit status: 101",
                workspace_root.display()
            )
        );
    }

    fn initialized_service(fixture: &CrateFixture) -> (LspService<TestBackend>, ClientSocket) {
        let root = fixture.path("workspace");
        let workspace_folders = vec![root];
        let (service, socket) = LspService::new(|client| TestBackend {
            registry: EngineRegistry::new(
                client,
                workspace_folders.clone(),
                ServerConfig::from_engine_config(EngineConfig::default()),
                EditorStateHandle::default(),
                ClientStatusCapabilities::default(),
            ),
        });

        (service, socket)
    }

    #[derive(Debug)]
    struct TestBackend {
        registry: EngineRegistry,
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
