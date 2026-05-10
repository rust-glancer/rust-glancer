use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;
use rg_lsp_proto::{AnalysisConfig, DiagnosticsConfig};
use tokio::sync::Mutex;
use tower_lsp_server::Client as LspClient;

use crate::{engine_client::EngineClient, engine_process::EngineProcess};

pub(crate) mod routing;
mod slot;
mod state;

use self::{
    routing::{EngineId, normalize_path},
    slot::{EngineEntry, EngineSlot},
    state::{EngineRegistryInner, EngineSpawnConfig, ReservedEngineRoute, ReservedEngineStart},
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
    pub(crate) fn new(lsp_client: LspClient) -> Self {
        Self {
            lsp_client,
            inner: Mutex::default(),
        }
    }

    /// Stores initialization configuration without starting analysis work yet.
    pub(crate) async fn initialize(
        &self,
        root: PathBuf,
        workspace_folders: Vec<PathBuf>,
        analysis_config: AnalysisConfig,
        diagnostics_config: DiagnosticsConfig,
    ) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().await;
            inner.initial_root = Some(normalize_path(root));
            inner.routing.set_workspace_folders(workspace_folders);
            inner.analysis_config = Some(analysis_config);
            inner.diagnostics_config = Some(diagnostics_config);
        }

        Ok(())
    }

    /// Starts the initial workspace engine after the LSP `initialized` notification.
    pub(crate) async fn start_initial_engine(&self) -> anyhow::Result<EngineClient> {
        let root = {
            let inner = self.inner.lock().await;
            inner
                .initial_root
                .clone()
                .context("while attempting to start engine before initial root")?
        };
        self.ensure_engine_for_root(root).await
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

    /// Finds or starts the engine that should receive a document-scoped request.
    pub(crate) async fn engine_for_document(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<EngineClient>> {
        let path = normalize_path(path);
        let route = {
            let mut inner = self.inner.lock().await;
            inner.route_document(&path)?
        };

        let Some(route) = route else {
            return Ok(None);
        };

        match route {
            ReservedEngineRoute::Existing(id) => self.engine_for_existing_id(id).await,
            ReservedEngineRoute::Spawn(start) => self.start_reserved_engine(start).await.map(Some),
        }
    }

    /// Ensures that the given Cargo root has a ready engine and returns its RPC client.
    async fn ensure_engine_for_root(&self, root: PathBuf) -> anyhow::Result<EngineClient> {
        let route = {
            let mut inner = self.inner.lock().await;
            inner.route_root(root)?
        };

        match route {
            ReservedEngineRoute::Existing(id) => self
                .engine_for_existing_id(id)
                .await?
                .context("while attempting to find reserved engine client"),
            ReservedEngineRoute::Spawn(start) => self.start_reserved_engine(start).await,
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
                let mut inner = self.inner.lock().await;
                match inner.engine(id) {
                    Some(EngineSlot::Ready(engine)) => {
                        let engine_client = engine.process.engine_client().clone();
                        inner.routing.set_active_id(id);
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
        let notify = {
            let mut inner = self.inner.lock().await;
            let notify = inner
                .engine(id)
                .and_then(EngineSlot::notify)
                .expect("reserved engine slot should be starting");
            inner.engines[id.index()] = EngineSlot::Ready(EngineEntry { process });
            inner.routing.set_active_id(id);
            notify
        };
        notify.notify_waiters();
    }

    /// Replaces a starting slot with a failure and wakes waiters.
    async fn mark_failed(&self, id: EngineId, root: PathBuf, error: String) {
        let notify = {
            let mut inner = self.inner.lock().await;
            let notify = inner.engine(id).and_then(EngineSlot::notify);
            inner.engines[id.index()] = EngineSlot::Failed {
                root,
                error: Arc::from(error),
            };
            notify
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }

    /// Spawns the engine subprocess and sends its protocol initialize request.
    async fn spawn_engine(
        &self,
        root: PathBuf,
        config: EngineSpawnConfig,
    ) -> anyhow::Result<EngineProcess> {
        let engine = EngineProcess::spawn(self.lsp_client.clone()).await?;
        let engine_client = engine.engine_client().clone();
        let initialize_root = root.clone();
        let analysis = config.analysis;
        let diagnostics = config.diagnostics;
        engine_client
            .call(
                "initialize",
                move |engine_client, request_context| async move {
                    engine_client
                        .initialize(
                            request_context,
                            initialize_root,
                            analysis.package_residency_policy,
                            analysis.cargo_metadata_config,
                            diagnostics,
                        )
                        .await
                },
            )
            .await?;

        tracing::info!(root = %root.display(), "started rust-glancer engine");
        Ok(engine)
    }
}
