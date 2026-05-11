use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_lsp_proto::EngineConfig;
use tokio::sync::Mutex;
use tower_lsp_server::Client as LspClient;

use crate::{
    client_notifications::{ActiveWorkspaceChanged, ActiveWorkspaceStatus},
    engine_client::EngineClient,
    engine_process::EngineProcess,
};

mod document_owner;
pub(crate) mod routing;
mod slot;
mod state;

use self::{
    document_owner::{DocumentOwner, OpenFileCachePolicy},
    routing::{EngineId, normalize_path},
    slot::{EngineEntry, EngineSlot},
    state::{EngineRegistryInner, ReservedEngineRoute, ReservedEngineStart},
};

/// Routes LSP requests to the engine process that owns the requested file.
///
/// The server process is the only place that knows about multiple engines. Routing owns path/root
/// decisions, while the registry owns engine lifecycle slots and RPC clients.
#[derive(Debug)]
pub(crate) struct EngineRegistry {
    lsp_client: LspClient,
    inner: Mutex<EngineRegistryInner>,
}

impl EngineRegistry {
    /// Creates a registry that can spawn engines and forward their notifications to the LSP client.
    pub(crate) fn new(
        lsp_client: LspClient,
        workspace_folders: Vec<PathBuf>,
        config: EngineConfig,
    ) -> Self {
        Self {
            lsp_client,
            inner: Mutex::new(EngineRegistryInner::new(workspace_folders, config)),
        }
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
    pub(crate) async fn active_engine(&self) -> Option<EngineClient> {
        let inner = self.inner.lock().await;
        let id = inner.routing.active_id()?;
        inner
            .engine(id)
            .and_then(EngineSlot::ready)
            .map(|engine| engine.process.engine_client().clone())
    }

    /// Routes a newly opened document and records exact file ownership until `didClose`.
    pub(crate) async fn open_document(&self, path: &Path) -> anyhow::Result<Option<EngineClient>> {
        let path = normalize_path(path);
        let owner = {
            let mut inner = self.inner.lock().await;
            DocumentOwner::new(&mut inner, &path, OpenFileCachePolicy::Record)?
        };
        let Some(owner) = owner else {
            return Ok(None);
        };

        let id = owner.id();
        match self.engine_for_document_owner(owner).await {
            Ok(Some(engine_client)) => Ok(Some(engine_client)),
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

    /// Finds or starts the engine that should receive a document-scoped request.
    pub(crate) async fn document(&self, path: &Path) -> anyhow::Result<Option<EngineClient>> {
        let path = normalize_path(path);
        let owner = {
            let mut inner = self.inner.lock().await;
            DocumentOwner::new(&mut inner, &path, OpenFileCachePolicy::Ignore)?
        };
        let Some(owner) = owner else {
            return Ok(None);
        };

        self.engine_for_document_owner(owner).await
    }

    /// Routes a closing document to its cached owner and forgets that ownership.
    pub(crate) async fn close_document(&self, path: &Path) -> anyhow::Result<Option<EngineClient>> {
        let path = normalize_path(path);
        let owner = {
            let mut inner = self.inner.lock().await;
            DocumentOwner::new(&mut inner, &path, OpenFileCachePolicy::Remove)?
        };
        let Some(owner) = owner else {
            return Ok(None);
        };

        self.engine_for_document_owner(owner).await
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

        self.publish_active_workspace(status).await;
    }

    async fn publish_active_workspace(&self, status: Option<ActiveWorkspaceStatus>) {
        if let Some(status) = status {
            self.lsp_client
                .send_notification::<ActiveWorkspaceChanged>(ActiveWorkspaceChanged::params(
                    &status,
                ))
                .await;
        }
    }

    /// Materializes a reserved engine id into a ready engine process.
    async fn start_reserved_engine(
        &self,
        start: ReservedEngineStart,
    ) -> anyhow::Result<EngineClient> {
        let engine = match self.spawn_engine(start.root.clone(), start.config).await {
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
            .call("initialized", |engine_client, request_context| async move {
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

        self.mark_ready(start.id, engine).await;
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
                    Some(EngineSlot::Starting { notify, .. }) => Some(notify.clone()),
                    Some(EngineSlot::Failed { root, error }) => {
                        return Err(anyhow::anyhow!(
                            "rust-glancer engine for `{}` failed to start: {error}",
                            root.display()
                        ));
                    }
                    None => return Ok(None),
                }
            };

            // Existing ids can point at a reserved-but-not-ready slot. Wait outside the registry
            // lock so the task that is starting the engine can mark the slot ready or failed.
            wait.expect("starting engine should provide notification")
                .notified()
                .await;
        }
    }

    /// Replaces a starting slot with a ready process and wakes waiters.
    async fn mark_ready(&self, id: EngineId, process: EngineProcess) {
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
        self.publish_active_workspace(status).await;
    }

    /// Replaces a starting slot with a failure and wakes waiters.
    async fn mark_failed(&self, id: EngineId, root: PathBuf, error: String) {
        let (notify, status) = {
            let mut inner = self.inner.lock().await;
            let notify = inner.engine(id).and_then(EngineSlot::notify);
            inner.engines[id.index()] = EngineSlot::Failed {
                root,
                error: Arc::from(error),
            };
            let status = inner.workspace_status_update();
            (notify, status)
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
        self.publish_active_workspace(status).await;
    }

    /// Spawns the engine subprocess and sends its protocol initialize request.
    async fn spawn_engine(
        &self,
        root: PathBuf,
        config: EngineConfig,
    ) -> anyhow::Result<EngineProcess> {
        let engine = EngineProcess::spawn(self.lsp_client.clone()).await?;
        let engine_client = engine.engine_client().clone();
        let initialize_root = root.clone();
        engine_client
            .call(
                "initialize",
                move |engine_client, request_context| async move {
                    engine_client
                        .initialize(request_context, initialize_root, config)
                        .await
                },
            )
            .await?;

        tracing::info!(root = %root.display(), "started rust-glancer engine");
        Ok(engine)
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

    use crate::client_notifications::ActiveWorkspaceState;

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
    async fn open_document_records_owner_before_engine_startup_completes() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("workspace/project_a/src/lib.rs");

        let owner = {
            let mut inner = registry.inner.lock().await;
            DocumentOwner::new(&mut inner, &document, OpenFileCachePolicy::Record)
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
    async fn unopened_document_route_does_not_populate_open_file_cache() {
        let fixture = fixture_crate(WORKSPACE_FIXTURE);
        let (service, _socket) = initialized_service(&fixture);
        let registry = &service.inner().registry;
        let document = fixture.path("workspace/project_a/src/lib.rs");

        let owner = {
            let mut inner = registry.inner.lock().await;
            DocumentOwner::new(&mut inner, &document, OpenFileCachePolicy::Ignore)
                .expect("document request should route through Cargo workspace")
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
        assert_eq!(cached_owner, None);
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
            DocumentOwner::new(&mut inner, &document, OpenFileCachePolicy::Ignore)
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
            DocumentOwner::new(&mut inner, &document, OpenFileCachePolicy::Record)
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

    fn initialized_service(fixture: &CrateFixture) -> (LspService<TestBackend>, ClientSocket) {
        let root = fixture.path("workspace");
        let workspace_folders = vec![root];
        let (service, socket) = LspService::new(|client| TestBackend {
            registry: EngineRegistry::new(
                client,
                workspace_folders.clone(),
                EngineConfig::default(),
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
