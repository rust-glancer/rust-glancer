use std::path::PathBuf;

use rg_lsp_proto::{EngineConfig, EngineError, EngineResult, EngineService};
use tarpc::context;

use crate::engine::EngineCommand;

use super::Service;

/// Tarpc-facing engine API implementation.
///
/// This module is the translation layer from protocol-shaped requests into the current in-process
/// analysis worker and diagnostics handle. Keeping it separate makes the service state easier to
/// read without hiding the fact that this is still one façade over two internal subsystems.
impl EngineService for Service {
    async fn initialize(
        self,
        _: context::Context,
        root: PathBuf,
        config: EngineConfig,
    ) -> EngineResult<()> {
        self.diagnostics
            .configure(root.clone(), config.diagnostics)
            .await;
        self.engine
            .request(|respond_to| EngineCommand::Initialize {
                root,
                analysis: config.analysis,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn initialized(self, _: context::Context) -> EngineResult<()> {
        self.diagnostics.launch_on_startup().await;
        Ok(())
    }

    async fn did_open(
        self,
        _: context::Context,
        path: PathBuf,
        version: Option<i32>,
        text: String,
    ) -> EngineResult<()> {
        let text_len = text.len();
        self.engine
            .documents
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

        Ok(())
    }

    async fn did_change(
        self,
        _: context::Context,
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<String>,
        content_change_count: usize,
    ) -> EngineResult<()> {
        let full_text_len = full_text.as_deref().map(str::len);
        let mut documents = self.engine.documents.lock().await;
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
            self.engine.refresh_inlay_hints();
        }

        Ok(())
    }

    async fn did_save(
        self,
        _: context::Context,
        path: PathBuf,
        text: Option<String>,
    ) -> EngineResult<()> {
        self.diagnostics.launch_on_save(path.clone()).await;
        let saved_text_len = text.as_ref().map(String::len);
        let mut documents = self.engine.documents.lock().await;
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
            .engine
            .request(|respond_to| EngineCommand::DidSave {
                path,
                text,
                respond_to,
            })
            .await
        {
            self.engine
                .mark_dirty_after_failed_save(saved_path, error)
                .await;
            return Ok(());
        }

        self.engine.log_freshness_after_save(&saved_path).await;
        self.engine.refresh_inlay_hints();

        Ok(())
    }

    async fn did_close(self, _: context::Context, path: PathBuf) -> EngineResult<()> {
        let mut documents = self.engine.documents.lock().await;
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

        Ok(())
    }

    async fn goto_definition(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::GotoDefinition {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn goto_type_definition(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::GotoTypeDefinition {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn goto_implementation(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::GotoImplementation {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn references(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
        include_declaration: bool,
    ) -> EngineResult<Vec<ls_types::Location>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::References {
                path,
                position,
                include_declaration,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn document_highlight(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::DocumentHighlight>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::DocumentHighlight {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn hover(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Option<ls_types::Hover>> {
        if self.engine.is_dirty(&path).await {
            return Ok(None);
        }

        self.engine
            .request(|respond_to| EngineCommand::Hover {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn completion(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::CompletionItem>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::Completion {
                path,
                position,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn document_symbol(
        self,
        _: context::Context,
        path: PathBuf,
    ) -> EngineResult<Vec<ls_types::DocumentSymbol>> {
        let freshness = self.engine.documents.lock().await.freshness(&path);
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

        self.engine
            .request(|respond_to| EngineCommand::DocumentSymbol { path, respond_to })
            .await
            .map_err(EngineError::from)
    }

    async fn inlay_hint(
        self,
        _: context::Context,
        path: PathBuf,
        range: ls_types::Range,
    ) -> EngineResult<Vec<ls_types::InlayHint>> {
        if self.engine.is_dirty(&path).await {
            return Ok(Vec::new());
        }

        self.engine
            .request(|respond_to| EngineCommand::InlayHint {
                path,
                range,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn workspace_symbol(
        self,
        _: context::Context,
        query: String,
    ) -> EngineResult<Vec<ls_types::WorkspaceSymbol>> {
        self.engine
            .request(|respond_to| EngineCommand::WorkspaceSymbol { query, respond_to })
            .await
            .map_err(EngineError::from)
    }

    async fn reindex_workspace(self, _: context::Context) -> EngineResult<()> {
        self.engine
            .request(|respond_to| EngineCommand::ReindexWorkspace { respond_to })
            .await
            .map_err(EngineError::from)
    }

    async fn shutdown(self, _: context::Context) -> EngineResult<()> {
        self.diagnostics.shutdown().await;
        self.engine
            .request(EngineCommand::Shutdown)
            .await
            .map_err(EngineError::from)
    }
}
