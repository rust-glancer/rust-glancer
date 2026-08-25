//! RPC-facing service façade for one analysis engine process.
//!
//! The generated protocol trait translates immutable RPC inputs into work for two long-lived
//! subsystems. `EngineHandle` serializes semantic project work, while `DiagnosticsHandle` runs
//! Cargo diagnostics independently and publishes side effects through the notification channel.
//! This service does not own or recapture editor lifecycle state.

mod notifications;

use std::{path::PathBuf, sync::Arc};

use anyhow::Context as _;
use rg_lsp_proto::{
    DocumentPositionSnapshot, DocumentRangeSnapshot, EditorDocumentSnapshot, EngineConfig,
    EngineError, EngineResult, EngineService, GlobalPositionSnapshot, QueryError, QueryValue,
    SaveProposal, SavedProjectChanges,
};
use rg_project::SavedFileChange;
use rg_source::CapturedSource;
use tarpc::context;

pub use self::notifications::ServiceNotificationsSink;
use crate::{
    diagnostics::DiagnosticsHandle,
    engine::{EngineCommand, EngineHandle, ProjectConfiguration},
    memory::MemoryControl,
};

#[cfg(test)]
pub(crate) use self::notifications::ServiceNotificationPublisher;

/// RPC-facing façade owned by one engine process.
///
/// `Service` is the boundary visible to the LSP server: it accepts immutable query inputs and
/// coordinates analysis and Cargo diagnostics without owning editor lifecycle state.
#[derive(Clone, Debug)]
pub struct Service {
    engine: EngineHandle,
    diagnostics: DiagnosticsHandle,
}

impl Service {
    pub fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        let engine = EngineHandle::spawn(memory_control, notifications.clone());
        let diagnostics = DiagnosticsHandle::new(notifications);

        Self {
            engine,
            diagnostics,
        }
    }
}

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
                configuration: ProjectConfiguration::from(analysis),
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn initialized(self, _: context::Context) -> EngineResult<()> {
        self.diagnostics.launch_on_startup().await;
        Ok(())
    }

    async fn set_deferred_indexing_priority(
        self,
        _: context::Context,
        path: PathBuf,
        prioritized: bool,
    ) -> EngineResult<()> {
        self.engine
            .request(|respond_to| EngineCommand::SetDeferredIndexingPriority {
                path,
                prioritized,
                respond_to,
            })
            .await
            .map_err(EngineError::from)
    }

    async fn did_save(self, _: context::Context, proposal: SaveProposal) -> EngineResult<u64> {
        let client_version = proposal.client_version();
        let (target, text) = proposal.into_parts();
        let path = target.path().to_path_buf();
        tracing::trace!(
            path = %path.display(),
            session = target.session().get(),
            revision = target.revision().get(),
            ?client_version,
            captured_text_len = text.len(),
            "submitting ingress-captured save proposal"
        );

        let change = if path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
            let captured = CapturedSource::new(&path, text)
                .context("prepare captured editor saved Rust source")
                .map_err(EngineError::from)?;
            SavedFileChange::captured(captured)
        } else {
            // Manifests and graph-shaped inputs keep the path transaction because Cargo metadata
            // remains their source boundary.
            SavedFileChange::fs_path(&path)
        };
        let generation = self
            .engine
            .request(|respond_to| EngineCommand::SavedProjectChanges {
                changes: vec![change],
                respond_to,
            })
            .await
            .map_err(EngineError::from)?;

        // Start diagnostics only after the saved-project candidate has published successfully.
        self.diagnostics.launch_on_editor_save(path).await;

        Ok(generation)
    }

    async fn external_project_changes(
        self,
        _: context::Context,
        changes: SavedProjectChanges,
    ) -> EngineResult<()> {
        let (captured_sources, fs_paths) = changes.into_parts();
        let diagnostics_path = captured_sources
            .first()
            .map(|source| source.path().to_path_buf())
            .or_else(|| fs_paths.first().cloned());
        let mut project_changes = Vec::with_capacity(captured_sources.len() + fs_paths.len());
        for input in captured_sources {
            let (path, text) = input.into_parts();
            let captured = CapturedSource::new(path, text)
                .context("prepare captured external saved Rust source")
                .map_err(EngineError::from)?;
            project_changes.push(SavedFileChange::captured(captured));
        }
        project_changes.extend(fs_paths.into_iter().map(SavedFileChange::fs_path));

        let changed_file_count = project_changes.len();
        let generation = self
            .engine
            .request(|respond_to| EngineCommand::SavedProjectChanges {
                changes: project_changes,
                respond_to,
            })
            .await
            .map_err(EngineError::from)?;

        if let Some(path) = diagnostics_path {
            self.diagnostics.launch_on_external_change(path).await;
        }
        tracing::debug!(
            changed_files = changed_file_count,
            saved_project_generation = generation,
            "applied external project path changes"
        );
        self.engine.refresh_inlay_hints();
        Ok(())
    }

    async fn goto_definition(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::GotoDefinition { input, respond_to })
            .await
    }

    async fn goto_type_definition(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::GotoTypeDefinition { input, respond_to })
            .await
    }

    async fn goto_implementation(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::GotoImplementation { input, respond_to })
            .await
    }

    async fn references(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
        include_declaration: bool,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::References {
                input,
                include_declaration,
                respond_to,
            })
            .await
    }

    async fn prepare_rename(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Option<ls_types::PrepareRenameResponse>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::PrepareRename { input, respond_to })
            .await
    }

    async fn rename(
        self,
        _: context::Context,
        input: GlobalPositionSnapshot,
        new_name: String,
    ) -> Result<QueryValue<Option<ls_types::WorkspaceEdit>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::Rename {
                input,
                new_name,
                respond_to,
            })
            .await
    }

    async fn document_highlight(
        self,
        _: context::Context,
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::DocumentHighlight>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::DocumentHighlight { input, respond_to })
            .await
    }

    async fn hover(
        self,
        _: context::Context,
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Option<ls_types::Hover>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::Hover { input, respond_to })
            .await
    }

    async fn code_action(
        self,
        _: context::Context,
        input: DocumentRangeSnapshot,
        request_context: rg_lsp_proto::CodeActionRequestContext,
    ) -> Result<QueryValue<Vec<ls_types::CodeAction>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::CodeAction {
                input,
                request_context,
                respond_to,
            })
            .await
    }

    async fn completion(
        self,
        _: context::Context,
        input: DocumentPositionSnapshot,
        client_capabilities: rg_lsp_proto::CompletionClientCapabilities,
    ) -> Result<QueryValue<Vec<ls_types::CompletionItem>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::Completion {
                input,
                client_capabilities,
                respond_to,
            })
            .await
    }

    async fn formatting(
        self,
        _: context::Context,
        snapshot: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Option<Vec<ls_types::TextEdit>>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::Formatting {
                snapshot,
                respond_to,
            })
            .await
    }

    async fn document_symbol(
        self,
        _: context::Context,
        snapshot: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::DocumentSymbol>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::DocumentSymbol {
                snapshot,
                respond_to,
            })
            .await
    }

    async fn inlay_hint(
        self,
        _: context::Context,
        input: DocumentRangeSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::InlayHint>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::InlayHint { input, respond_to })
            .await
    }

    async fn workspace_symbol(
        self,
        _: context::Context,
        query: String,
    ) -> Result<QueryValue<Vec<ls_types::WorkspaceSymbol>>, QueryError> {
        self.engine
            .query(|respond_to| EngineCommand::WorkspaceSymbol { query, respond_to })
            .await
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
