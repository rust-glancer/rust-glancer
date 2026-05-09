mod command;
mod worker;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
};

use anyhow::Context as _;
use rg_project::PackageResidencyPolicy;
use rg_workspace::CargoMetadataConfig;
use tokio::sync::{Mutex, oneshot};

use self::{
    command::{EngineCommand, EngineResponse},
    worker::EngineWorker,
};
use crate::{
    EngineEvent, EngineLogLevel,
    check::{CheckConfig, CheckHandle},
    documents::DocumentStore,
    events::EngineEventSink,
    memory::MemoryControl,
};

#[derive(Clone, Debug)]
pub struct EngineHandle {
    sender: Sender<EngineCommand>,
    documents: Arc<Mutex<DocumentStore>>,
    check: CheckHandle,
    events: EngineEventSink,
}

impl EngineHandle {
    /// Starts the in-process workspace engine and wires it to the LSP event sink.
    ///
    /// The public handle already owns document freshness and cargo diagnostics state. Keeping that
    /// state behind this boundary makes the future subprocess engine a transport swap rather than
    /// a reshuffle of LSP-facing code.
    pub fn spawn(memory_control: Arc<dyn MemoryControl>, events: EngineEventSink) -> Self {
        let (sender, receiver) = mpsc::channel();
        let documents = Arc::new(Mutex::new(DocumentStore::default()));
        let check = CheckHandle::new(events.clone(), Arc::clone(&documents));

        thread::spawn(move || EngineWorker::new(memory_control).run(receiver));

        Self {
            sender,
            documents,
            check,
            events,
        }
    }

    pub async fn initialize(
        &self,
        root: PathBuf,
        package_residency_policy: PackageResidencyPolicy,
        cargo_metadata_config: CargoMetadataConfig,
        check_config: CheckConfig,
    ) -> anyhow::Result<()> {
        self.check.configure(root.clone(), check_config).await;
        self.request(|respond_to| EngineCommand::Initialize {
            root,
            package_residency_policy,
            cargo_metadata_config,
            respond_to,
        })
        .await
    }

    pub async fn launch_check_on_startup(&self) {
        self.check.launch_on_startup().await;
    }

    pub async fn did_open(&self, path: PathBuf, version: Option<i32>, text: &str) {
        let text_len = text.len();
        self.documents
            .lock()
            .await
            .did_open(path.clone(), version, text);

        tracing::debug!(path = %path.display(), "opened clean document snapshot");
        tracing::trace!(
            path = %path.display(),
            version,
            text_len,
            "recorded open document freshness"
        );
    }

    pub async fn did_change(
        &self,
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<String>,
        content_change_count: usize,
    ) {
        let full_text_len = full_text.as_deref().map(str::len);
        let mut documents = self.documents.lock().await;
        let change = documents.did_change(path.clone(), version, full_text.as_deref());
        let freshness = documents.freshness(&path);
        drop(documents);

        tracing::debug!(
            path = %path.display(),
            became_dirty = change.became_dirty,
            became_clean = change.became_clean,
            dirty = freshness.dirty(),
            "updated document freshness after change"
        );
        tracing::trace!(
            path = %path.display(),
            version,
            content_changes = content_change_count,
            full_text_len,
            tracked = freshness.tracked(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness after change"
        );

        if change.became_dirty || change.became_clean {
            self.events.send(EngineEvent::InlayHintRefresh);
        }
    }

    pub async fn did_save(&self, path: PathBuf, text: Option<String>) {
        let saved_text_len = text.as_ref().map(String::len);
        let mut documents = self.documents.lock().await;
        documents.did_save(path.clone(), text.as_deref());
        let freshness = documents.freshness(&path);
        drop(documents);

        tracing::debug!(path = %path.display(), "marked document clean before save reindex");
        tracing::trace!(
            path = %path.display(),
            saved_text_len,
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness before save reindex"
        );

        self.check.launch_on_save(path.clone()).await;

        let saved_path = path.clone();
        if let Err(error) = self
            .request(|respond_to| EngineCommand::DidSave {
                path,
                text,
                respond_to,
            })
            .await
        {
            self.mark_dirty_after_failed_save(saved_path, error).await;
            return;
        }

        self.log_freshness_after_save(&saved_path).await;
        self.events.send(EngineEvent::InlayHintRefresh);
    }

    pub async fn did_close(&self, path: PathBuf) {
        let mut documents = self.documents.lock().await;
        let freshness = documents.freshness(&path);
        documents.did_close(&path);
        drop(documents);

        tracing::debug!(path = %path.display(), "closed document");
        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "removed document freshness"
        );
    }

    pub async fn goto_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        if self.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.request(|respond_to| EngineCommand::GotoDefinition {
            path,
            position,
            respond_to,
        })
        .await
    }

    pub async fn goto_type_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        if self.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.request(|respond_to| EngineCommand::GotoTypeDefinition {
            path,
            position,
            respond_to,
        })
        .await
    }

    pub async fn hover(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Option<ls_types::Hover>> {
        if self.is_dirty(&path).await {
            return Ok(None);
        }

        self.request(|respond_to| EngineCommand::Hover {
            path,
            position,
            respond_to,
        })
        .await
    }

    pub async fn completion(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::CompletionItem>> {
        if self.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.request(|respond_to| EngineCommand::Completion {
            path,
            position,
            respond_to,
        })
        .await
    }

    pub async fn document_symbol(
        &self,
        path: PathBuf,
    ) -> anyhow::Result<Vec<ls_types::DocumentSymbol>> {
        let freshness = self.documents.lock().await.freshness(&path);
        if freshness.dirty() {
            // LSP has refresh requests for features like inlay hints, but not for document symbols.
            // Returning an empty symbol tree while the document is dirty can leave VS Code's Outline
            // empty after save, so document symbols intentionally use the last saved snapshot.
            // TODO: This can show stale ranges while the dirty buffer shifts item spans. VSCode has
            // an open issue to trigger outline refresh:
            // https://github.com/microsoft/vscode/issues/108722
            tracing::trace!(
                path = %path.display(),
                tracked = freshness.tracked(),
                version = ?freshness.version(),
                saved_len = ?freshness.saved_len(),
                live_len = ?freshness.live_len(),
                saved_hash = ?freshness.saved_hash(),
                live_hash = ?freshness.live_hash(),
                "document symbol request is using saved snapshot for dirty document"
            );
        }

        self.request(|respond_to| EngineCommand::DocumentSymbol { path, respond_to })
            .await
    }

    pub async fn inlay_hint(
        &self,
        path: PathBuf,
        range: ls_types::Range,
    ) -> anyhow::Result<Vec<ls_types::InlayHint>> {
        if self.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.request(|respond_to| EngineCommand::InlayHint {
            path,
            range,
            respond_to,
        })
        .await
    }

    pub async fn workspace_symbol(
        &self,
        query: String,
    ) -> anyhow::Result<Vec<ls_types::WorkspaceSymbol>> {
        self.request(|respond_to| EngineCommand::WorkspaceSymbol { query, respond_to })
            .await
    }

    pub async fn reindex_workspace(&self) -> anyhow::Result<()> {
        self.request(|respond_to| EngineCommand::ReindexWorkspace { respond_to })
            .await
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.check.shutdown().await;
        self.request(EngineCommand::Shutdown).await
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(EngineResponse<T>) -> EngineCommand,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        let (respond_to, response) = oneshot::channel();
        self.sender
            .send(build(respond_to))
            .context("while attempting to send LSP engine command")?;

        response
            .await
            .context("while attempting to receive LSP engine response")?
    }

    async fn is_dirty(&self, path: &Path) -> bool {
        let freshness = self.documents.lock().await.freshness(path);
        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "checked document freshness"
        );

        if freshness.dirty() {
            tracing::debug!(
                path = %path.display(),
                "returning empty result for dirty document"
            );
        }

        freshness.dirty()
    }

    async fn mark_dirty_after_failed_save(&self, path: PathBuf, error: anyhow::Error) {
        let mut documents = self.documents.lock().await;
        documents.mark_dirty_after_failed_save(path.clone());
        let freshness = documents.freshness(&path);
        drop(documents);

        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness after failed save reindex"
        );

        let message = format!("failed to process saved file: {error}");
        self.events.send(EngineEvent::LogMessage {
            level: EngineLogLevel::Error,
            message,
        });
    }

    async fn log_freshness_after_save(&self, path: &Path) {
        let freshness = self.documents.lock().await.freshness(path);
        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness after save reindex"
        );
    }
}
