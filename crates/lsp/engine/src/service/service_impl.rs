use std::path::PathBuf;

use rg_lsp_proto::{EngineConfig, EngineError, EngineResult, EngineService};
use tarpc::context;

use crate::{documents::DirtyDocumentSnapshotState, engine::EngineCommand};

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
        let analysis = config.analysis;
        self.diagnostics
            .configure(root.clone(), config.diagnostics, analysis.clone())
            .await;
        self.engine
            .request(|respond_to| EngineCommand::Initialize {
                root,
                analysis,
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
        let mut documents = self.engine.documents.lock().await;
        documents.did_open(path.clone(), version, &text);
        let dirty = documents.dirty_snapshot(&path);
        self.engine.sync_dirty_state(&path, &dirty);
        drop(documents);

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
        let dirty = documents.dirty_snapshot(&path);
        self.engine.sync_dirty_state(&path, &dirty);
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

        self.engine.refresh_inlay_hints_debounced();

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
        let dirty = documents.dirty_snapshot(&path);
        self.engine.sync_dirty_state(&path, &dirty);
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
            .request(|respond_to| EngineCommand::ProjectPathsChanged {
                paths: vec![path],
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
        self.engine.refresh_inlay_hints_now();

        Ok(())
    }

    async fn external_project_paths_changed(
        self,
        _: context::Context,
        paths: Vec<PathBuf>,
    ) -> EngineResult<()> {
        let mut forwarded_paths = Vec::new();
        let mut changed_texts = Vec::new();

        // Watched-file notifications only carry paths. Read the files here so document state and
        // project state are updated from the same disk text.
        for path in paths {
            if !path.is_file() {
                tracing::trace!(
                    path = %path.display(),
                    "external project path skipped because path is not a file"
                );
                continue;
            }

            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    tracing::debug!(
                        path = %path.display(),
                        error = %error,
                        "external project path skipped because source text could not be read"
                    );
                    continue;
                }
            };

            changed_texts.push((path.clone(), text));
            forwarded_paths.push(path);
        }

        if forwarded_paths.is_empty() {
            return Ok(());
        }

        // Update document state before asking the project to rebuild from these files.
        let mut documents = self.engine.documents.lock().await;
        for (path, text) in &changed_texts {
            documents.external_saved_change(path.clone(), text);
            let freshness = documents.freshness(path);
            let dirty = documents.dirty_snapshot(path);
            self.engine.sync_dirty_state(path, &dirty);

            tracing::trace!(
                path = %path.display(),
                tracked = freshness.tracked(),
                version = ?freshness.version(),
                dirty = freshness.dirty(),
                saved_len = ?freshness.saved_len(),
                live_len = ?freshness.live_len(),
                saved_hash = ?freshness.saved_hash(),
                live_hash = ?freshness.live_hash(),
                "document freshness after external change"
            );
        }
        drop(documents);

        // Diagnostics run as a workspace command, so one changed path is enough to start them.
        if let Some(path) = forwarded_paths.first().cloned() {
            self.diagnostics.launch_on_save(path).await;
        }

        // The project decides whether a path is a source edit or a Cargo graph change.
        let changed_file_count = forwarded_paths.len();
        self.engine
            .request(|respond_to| EngineCommand::ProjectPathsChanged {
                paths: forwarded_paths,
                respond_to,
            })
            .await
            .map_err(EngineError::from)?;

        tracing::debug!(
            changed_files = changed_file_count,
            "applied external project path changes"
        );
        self.engine.refresh_inlay_hints_now();

        Ok(())
    }

    async fn did_close(self, _: context::Context, path: PathBuf) -> EngineResult<()> {
        let mut documents = self.engine.documents.lock().await;
        let freshness = documents.freshness(&path);
        documents.did_close(&path);
        let dirty = documents.dirty_snapshot(&path);
        self.engine.sync_dirty_state(&path, &dirty);
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::GotoDefinition {
                path,
                position,
                dirty,
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::GotoTypeDefinition {
                path,
                position,
                dirty,
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::GotoImplementation {
                path,
                position,
                dirty,
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::References {
                path,
                position,
                include_declaration,
                dirty,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn prepare_rename(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Option<ls_types::PrepareRenameResponse>> {
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(None),
        };

        self.engine
            .request(|respond_to| EngineCommand::PrepareRename {
                path,
                position,
                dirty,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn rename(
        self,
        _: context::Context,
        path: PathBuf,
        position: ls_types::Position,
        new_name: String,
    ) -> EngineResult<Option<ls_types::WorkspaceEdit>> {
        let dirty = {
            // Technically we have a TOCTOU here, but if someone really will try to
            // do a rename while simulanteously editing multiple files... They probably
            // should stop doing weird things.
            let documents = self.engine.documents.lock().await;
            if documents.has_dirty_documents_except(&path) {
                return Err(EngineError::new(
                    "rename requires saving other dirty Rust documents first",
                ));
            }

            match documents.dirty_snapshot(&path) {
                DirtyDocumentSnapshotState::Clean => None,
                DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
                DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(None),
            }
        };

        self.engine
            .request(|respond_to| EngineCommand::Rename {
                path,
                position,
                new_name,
                dirty,
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::DocumentHighlight {
                path,
                position,
                dirty,
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
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(None),
        };

        self.engine
            .request(|respond_to| EngineCommand::Hover {
                path,
                position,
                dirty,
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
        client_capabilities: rg_lsp_proto::CompletionClientCapabilities,
    ) -> EngineResult<Vec<ls_types::CompletionItem>> {
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::Completion {
                path,
                position,
                client_capabilities,
                dirty,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn formatting(
        self,
        _: context::Context,
        path: PathBuf,
    ) -> EngineResult<Option<Vec<ls_types::TextEdit>>> {
        let text = {
            let documents = self.engine.documents.lock().await;
            let freshness = documents.freshness(&path);
            let text = documents.current_text(&path);

            tracing::trace!(
                path = %path.display(),
                tracked = freshness.tracked(),
                version = ?freshness.version(),
                dirty = freshness.dirty(),
                has_text = text.is_some(),
                "checked document text for formatting"
            );

            text
        };
        let Some(text) = text else {
            tracing::debug!(
                path = %path.display(),
                "formatting skipped because document has no live text"
            );
            return Ok(None);
        };

        self.engine
            .request(|respond_to| EngineCommand::Formatting {
                path,
                text,
                respond_to,
            })
            .await
            .map(Some)
            .map_err(EngineError::from)
    }

    async fn document_symbol(
        self,
        _: context::Context,
        path: PathBuf,
    ) -> EngineResult<Vec<ls_types::DocumentSymbol>> {
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => None,
        };

        self.engine
            .request(|respond_to| EngineCommand::DocumentSymbol {
                path,
                dirty,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn inlay_hint(
        self,
        _: context::Context,
        path: PathBuf,
        range: ls_types::Range,
    ) -> EngineResult<Vec<ls_types::InlayHint>> {
        let dirty = match self.engine.dirty_document_snapshot(&path).await {
            DirtyDocumentSnapshotState::Clean => None,
            DirtyDocumentSnapshotState::Dirty(dirty) => Some(dirty),
            DirtyDocumentSnapshotState::DirtyWithoutText => return Ok(Vec::new()),
        };

        self.engine
            .request(|respond_to| EngineCommand::InlayHint {
                path,
                range,
                dirty,
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
