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
use rg_lsp_proto::{
    CheckConfig, EngineEvent, EngineLogLevel, EngineNotifyFuture, EngineResultFuture,
    EngineService, EngineServiceHandle,
};
use rg_project::PackageResidencyPolicy;
use rg_workspace::CargoMetadataConfig;
use tokio::sync::{Mutex, oneshot};

use self::{
    command::{EngineCommand, EngineResponse},
    worker::EngineWorker,
};
use crate::{
    check::CheckHandle, documents::DocumentStore, events::EngineEventSink, memory::MemoryControl,
};

/// In-process engine façade used by the current LSP server.
///
/// The façade hides the fact that analysis requests and cargo diagnostics currently use different
/// internal mechanisms. The LSP server sees one engine service and a separate event stream.
#[derive(Clone, Debug)]
pub struct InProcessEngineService {
    analysis: InProcessAnalysisService,
    check: CheckHandle,
}

impl InProcessEngineService {
    pub fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        events: EngineEventSink,
    ) -> EngineServiceHandle {
        let documents = Arc::new(Mutex::new(DocumentStore::default()));
        let analysis =
            InProcessAnalysisService::spawn(memory_control, events.clone(), Arc::clone(&documents));
        let check = CheckHandle::new(events, documents);

        Arc::new(Self { analysis, check })
    }
}

#[derive(Clone, Debug)]
struct InProcessAnalysisService {
    sender: Sender<EngineCommand>,
    documents: Arc<Mutex<DocumentStore>>,
    events: EngineEventSink,
}

impl InProcessAnalysisService {
    /// Starts the current in-process worker behind the engine-service abstraction.
    fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        events: EngineEventSink,
        documents: Arc<Mutex<DocumentStore>>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || EngineWorker::new(memory_control).run(receiver));

        Self {
            sender,
            documents,
            events,
        }
    }

    async fn initialize(
        &self,
        root: PathBuf,
        package_residency_policy: PackageResidencyPolicy,
        cargo_metadata_config: CargoMetadataConfig,
    ) -> anyhow::Result<()> {
        self.request(|respond_to| EngineCommand::Initialize {
            root,
            package_residency_policy,
            cargo_metadata_config,
            respond_to,
        })
        .await
    }

    async fn did_open(&self, path: PathBuf, version: Option<i32>, text: String) {
        let text_len = text.len();
        self.documents
            .lock()
            .await
            .did_open(path.clone(), version, &text);

        tracing::debug!(path = %path.display(), "opened clean document snapshot");
        tracing::trace!(
            path = %path.display(),
            version,
            text_len,
            "recorded open document freshness"
        );
    }

    async fn did_change(
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

    async fn did_save(&self, path: PathBuf, text: Option<String>) {
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

    async fn did_close(&self, path: PathBuf) {
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

    async fn goto_definition(
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

    async fn goto_type_definition(
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

    async fn hover(
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

    async fn completion(
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

    async fn document_symbol(
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

    async fn inlay_hint(
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

    async fn workspace_symbol(
        &self,
        query: String,
    ) -> anyhow::Result<Vec<ls_types::WorkspaceSymbol>> {
        self.request(|respond_to| EngineCommand::WorkspaceSymbol { query, respond_to })
            .await
    }

    async fn reindex_workspace(&self) -> anyhow::Result<()> {
        self.request(|respond_to| EngineCommand::ReindexWorkspace { respond_to })
            .await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
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

impl EngineService for InProcessEngineService {
    fn initialize(
        &self,
        root: PathBuf,
        package_residency_policy: PackageResidencyPolicy,
        cargo_metadata_config: CargoMetadataConfig,
        check_config: CheckConfig,
    ) -> EngineResultFuture<'_, ()> {
        Box::pin(async move {
            self.check.configure(root.clone(), check_config).await;
            self.analysis
                .initialize(root, package_residency_policy, cargo_metadata_config)
                .await
        })
    }

    fn initialized(&self) -> EngineNotifyFuture<'_> {
        Box::pin(async move {
            self.check.launch_on_startup().await;
        })
    }

    fn did_open(
        &self,
        path: PathBuf,
        version: Option<i32>,
        text: String,
    ) -> EngineNotifyFuture<'_> {
        Box::pin(self.analysis.did_open(path, version, text))
    }

    fn did_change(
        &self,
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<String>,
        content_change_count: usize,
    ) -> EngineNotifyFuture<'_> {
        Box::pin(
            self.analysis
                .did_change(path, version, full_text, content_change_count),
        )
    }

    fn did_save(&self, path: PathBuf, text: Option<String>) -> EngineNotifyFuture<'_> {
        Box::pin(async move {
            self.check.launch_on_save(path.clone()).await;
            self.analysis.did_save(path, text).await;
        })
    }

    fn did_close(&self, path: PathBuf) -> EngineNotifyFuture<'_> {
        Box::pin(self.analysis.did_close(path))
    }

    fn goto_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::Location>> {
        Box::pin(self.analysis.goto_definition(path, position))
    }

    fn goto_type_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::Location>> {
        Box::pin(self.analysis.goto_type_definition(path, position))
    }

    fn hover(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Option<ls_types::Hover>> {
        Box::pin(self.analysis.hover(path, position))
    }

    fn completion(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::CompletionItem>> {
        Box::pin(self.analysis.completion(path, position))
    }

    fn document_symbol(
        &self,
        path: PathBuf,
    ) -> EngineResultFuture<'_, Vec<ls_types::DocumentSymbol>> {
        Box::pin(self.analysis.document_symbol(path))
    }

    fn inlay_hint(
        &self,
        path: PathBuf,
        range: ls_types::Range,
    ) -> EngineResultFuture<'_, Vec<ls_types::InlayHint>> {
        Box::pin(self.analysis.inlay_hint(path, range))
    }

    fn workspace_symbol(
        &self,
        query: String,
    ) -> EngineResultFuture<'_, Vec<ls_types::WorkspaceSymbol>> {
        Box::pin(self.analysis.workspace_symbol(query))
    }

    fn reindex_workspace(&self) -> EngineResultFuture<'_, ()> {
        Box::pin(self.analysis.reindex_workspace())
    }

    fn shutdown(&self) -> EngineResultFuture<'_, ()> {
        Box::pin(async move {
            self.check.shutdown().await;
            self.analysis.shutdown().await
        })
    }
}
