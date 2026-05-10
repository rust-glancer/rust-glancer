use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use rg_lsp_proto::{AnalysisConfig, DiagnosticsConfig};
use tokio::sync::Mutex;
use tower_lsp_server::Client;

use crate::{engine_client::EngineClient, engine_process::EngineProcess};

/// Routes LSP requests to the engine process that owns the requested file.
///
/// The server process is the only place that knows about multiple engines. Method handlers receive
/// exactly one routed `EngineClient`; the backend uses this registry to choose that client or to
/// fan out lifecycle notifications.
#[derive(Debug)]
pub(crate) struct EngineRegistry {
    client: Client,
    inner: Mutex<EngineRegistryInner>,
}

#[derive(Clone, Debug)]
pub(crate) struct UninitializedEngine {
    pub(crate) root: PathBuf,
    pub(crate) client: EngineClient,
}

impl EngineRegistry {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            inner: Mutex::default(),
        }
    }

    /// Stores initialization configuration and starts the first engine selected by LSP initialize.
    ///
    /// Lazy engines spawned later reuse the same configuration and, if the server has already
    /// received `initialized`, immediately receive that notification too.
    pub(crate) async fn initialize(
        &self,
        root: PathBuf,
        workspace_folders: Vec<PathBuf>,
        analysis_config: AnalysisConfig,
        diagnostics_config: DiagnosticsConfig,
    ) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().await;
            inner.workspace_folders = workspace_folders.into_iter().map(normalize_path).collect();
            inner.analysis_config = Some(analysis_config);
            inner.diagnostics_config = Some(diagnostics_config);
        }

        self.ensure_engine_for_root(normalize_path(root)).await?;
        Ok(())
    }

    /// Records that the LSP `initialized` notification has arrived and returns engines that still
    /// need the matching engine-side lifecycle notification.
    pub(crate) async fn uninitialized_engines(&self) -> Vec<UninitializedEngine> {
        let mut inner = self.inner.lock().await;
        inner.lsp_initialized = true;
        inner
            .engines
            .iter()
            .filter(|(_, engine)| !engine.initialized)
            .map(|(root, engine)| UninitializedEngine {
                root: root.clone(),
                client: engine.process.client().clone(),
            })
            .collect()
    }

    pub(crate) async fn mark_initialized(&self, root: &Path) {
        let mut inner = self.inner.lock().await;
        if let Some(engine) = inner.engines.get_mut(root) {
            engine.initialized = true;
        }
    }

    pub(crate) async fn engine_clients(&self) -> Vec<EngineClient> {
        let inner = self.inner.lock().await;
        inner
            .engines
            .values()
            .map(|engine| engine.process.client().clone())
            .collect()
    }

    pub(crate) async fn active_engine(&self) -> Option<EngineClient> {
        let inner = self.inner.lock().await;
        let root = inner.last_active_engine()?;
        inner
            .engines
            .get(&root)
            .map(|engine| engine.process.client().clone())
    }

    /// Finds or starts the engine that should receive a document-scoped request.
    pub(crate) async fn engine_for_document(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<EngineClient>> {
        let path = normalize_path(path);
        if let Some(client) = self.existing_engine_for_path(&path).await {
            return Ok(Some(client));
        }

        let spawn_root = {
            let inner = self.inner.lock().await;
            if inner.is_in_workspace(&path) {
                nearest_cargo_root(&path)
            } else {
                inner.last_active_engine()
            }
        };

        let Some(root) = spawn_root else {
            return Ok(None);
        };

        self.ensure_engine_for_root(root).await.map(Some)
    }

    async fn existing_engine_for_path(&self, path: &Path) -> Option<EngineClient> {
        let mut inner = self.inner.lock().await;
        let root = inner.engine_root_for_path(path)?;
        let engine = inner.engines.get(&root)?;
        let client = engine.process.client().clone();
        inner.last_active_root = Some(root);
        Some(client)
    }

    async fn ensure_engine_for_root(&self, root: PathBuf) -> anyhow::Result<EngineClient> {
        let config =
            {
                let mut inner = self.inner.lock().await;
                if let Some(engine) = inner.engines.get(&root) {
                    let client = engine.process.client().clone();
                    inner.last_active_root = Some(root);
                    return Ok(client);
                }

                EngineSpawnConfig {
                    analysis: inner.analysis_config.clone().context(
                        "while attempting to spawn engine before analysis configuration",
                    )?,
                    diagnostics: inner.diagnostics_config.clone().context(
                        "while attempting to spawn engine before diagnostics configuration",
                    )?,
                    lsp_initialized: inner.lsp_initialized,
                }
            };

        let lsp_initialized = config.lsp_initialized;
        let engine = self.spawn_engine(root.clone(), config).await?;
        let client = engine.client().clone();
        let mut initialized = false;
        if lsp_initialized {
            // Engines spawned after the LSP lifecycle has already advanced will never see another
            // client-side `initialized` notification, so complete that step before exposing them.
            client
                .call("initialized", |client, request_context| async move {
                    client.initialized(request_context).await
                })
                .await?;
            initialized = true;
        }

        let duplicate_client = {
            let mut inner = self.inner.lock().await;
            if let Some(existing) = inner.engines.get(&root) {
                let existing = existing.process.client().clone();
                inner.last_active_root = Some(root);
                Some(existing)
            } else {
                inner.engines.insert(
                    root.clone(),
                    EngineEntry {
                        process: engine,
                        initialized,
                    },
                );
                inner.last_active_root = Some(root);
                return Ok(client);
            }
        };

        // A concurrent request may have spawned the same root while this one was indexing.
        // Dropping the duplicate would kill it, but asking for a graceful shutdown keeps logs clean.
        client
            .notify("shutdown", |client, request_context| async move {
                client.shutdown(request_context).await
            })
            .await;

        Ok(duplicate_client.expect("duplicate engine should exist"))
    }

    async fn spawn_engine(
        &self,
        root: PathBuf,
        config: EngineSpawnConfig,
    ) -> anyhow::Result<EngineProcess> {
        let engine = EngineProcess::spawn(self.client.clone()).await?;
        let client = engine.client().clone();
        let initialize_root = root.clone();
        let analysis = config.analysis;
        let diagnostics = config.diagnostics;
        client
            .call("initialize", move |client, request_context| async move {
                client
                    .initialize(
                        request_context,
                        initialize_root,
                        analysis.package_residency_policy,
                        analysis.cargo_metadata_config,
                        diagnostics,
                    )
                    .await
            })
            .await?;

        tracing::info!(root = %root.display(), "started rust-glancer engine");
        Ok(engine)
    }
}

#[derive(Debug)]
struct EngineSpawnConfig {
    analysis: AnalysisConfig,
    diagnostics: DiagnosticsConfig,
    lsp_initialized: bool,
}

#[derive(Debug)]
struct EngineEntry {
    process: EngineProcess,
    initialized: bool,
}

#[derive(Debug, Default)]
struct EngineRegistryInner {
    workspace_folders: Vec<PathBuf>,
    engines: BTreeMap<PathBuf, EngineEntry>,
    last_active_root: Option<PathBuf>,
    analysis_config: Option<AnalysisConfig>,
    diagnostics_config: Option<DiagnosticsConfig>,
    lsp_initialized: bool,
}

impl EngineRegistryInner {
    fn engine_root_for_path(&self, path: &Path) -> Option<PathBuf> {
        matching_root_for_path(self.engines.keys(), path)
    }

    fn is_in_workspace(&self, path: &Path) -> bool {
        self.workspace_folders
            .iter()
            .any(|workspace_folder| path.starts_with(workspace_folder))
    }

    fn last_active_engine(&self) -> Option<PathBuf> {
        self.last_active_root
            .as_ref()
            .filter(|root| self.engines.contains_key(*root))
            .cloned()
    }
}

fn nearest_cargo_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.is_dir().then_some(path).or_else(|| path.parent());
    while let Some(candidate) = current {
        if candidate.join("Cargo.toml").is_file() {
            return Some(normalize_path(candidate));
        }
        current = candidate.parent();
    }
    None
}

fn matching_root_for_path<'a>(
    roots: impl Iterator<Item = &'a PathBuf>,
    path: &Path,
) -> Option<PathBuf> {
    roots
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .cloned()
}

fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{matching_root_for_path, nearest_cargo_root, normalize_path};

    #[test]
    fn nearest_cargo_root_finds_manifest_ancestor() {
        let root = std::env::temp_dir().join(format!(
            "rust-glancer-engine-registry-{}",
            std::process::id()
        ));
        let package = root.join("workspace/member");
        let source = package.join("src/lib.rs");
        fs::create_dir_all(source.parent().expect("source should have parent"))
            .expect("fixture dirs should be created");
        fs::write(package.join("Cargo.toml"), "[package]\nname = \"member\"\n")
            .expect("fixture manifest should be written");
        fs::write(&source, "").expect("fixture source should be written");

        assert_eq!(nearest_cargo_root(&source), Some(normalize_path(&package)));

        fs::remove_dir_all(root).expect("fixture root should be removed");
    }

    #[test]
    fn routing_prefers_longest_matching_root() {
        let outer = PathBuf::from("/workspace");
        let inner_root = PathBuf::from("/workspace/vendor/member");
        let roots = [outer, inner_root.clone()];

        assert_eq!(
            matching_root_for_path(
                roots.iter(),
                PathBuf::from("/workspace/vendor/member/src/lib.rs").as_path(),
            ),
            Some(inner_root),
        );
    }
}
