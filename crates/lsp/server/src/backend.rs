//! Transport-facing `LanguageServer` implementation.
//!
//! This file keeps the necessarily broad LSP trait implementation readable by delegating methods
//! with real behavior to the free-function modules under `methods`. It owns shared server services
//! and builds the narrow document or completion context required by each handler. Feature logic,
//! protocol conversion, and async open/save/close work stay in their narrower handler files.
//!
//! `didChange` is the deliberate exception. `EditorIngress` applies all of its edits to the stored
//! document before any async handler can start. The trait method therefore has no remaining work
//! to move into a separate handler file.

use std::{borrow::Cow, path::Path};

use anyhow::Context as _;
use tower_lsp_server::{
    Client as LspClient, LanguageServer,
    jsonrpc::{Error, ErrorCode, Result},
    ls_types::{request::*, *},
};

use rg_lsp_proto::ClientCapabilities as EngineClientCapabilities;
use rg_std::NormalizedPathBuf;
use tokio::sync::OnceCell;

use crate::{
    client_status::ClientStatusCapabilities,
    config::ServerConfig,
    engine_client::EngineClient,
    engine_registry::EngineRegistry,
    ingress::{self, EditorStateHandle},
    inlay_refresher::InlayRefresher,
    methods::{self, CompletionMethodContext, DocumentMethodContext},
    project_watcher::ProjectWatcher,
    recent_editor_saves::RecentEditorSaves,
};

#[derive(Debug)]
pub(crate) struct Backend {
    lsp_client: LspClient,
    engines: OnceCell<EngineRegistry>,
    project_watcher: OnceCell<ProjectWatcher>,
    client_capabilities: OnceCell<EngineClientCapabilities>,
    recent_editor_saves: RecentEditorSaves,
    editor: EditorStateHandle,
    inlay_refresher: InlayRefresher,
}

impl Backend {
    pub(crate) fn new(
        lsp_client: LspClient,
        recent_editor_saves: RecentEditorSaves,
        editor: EditorStateHandle,
        inlay_refresher: InlayRefresher,
    ) -> Self {
        Self {
            lsp_client,
            engines: OnceCell::new(),
            project_watcher: OnceCell::new(),
            client_capabilities: OnceCell::new(),
            recent_editor_saves,
            editor,
            inlay_refresher,
        }
    }

    async fn registry(&self) -> Result<&EngineRegistry> {
        self.engines.get().ok_or(Error {
            code: ErrorCode::ServerError(-32002),
            message: Cow::Borrowed("rust-glancer engine registry is not initialized"),
            data: None,
        })
    }

    /// Build the engine and ingress capture required by a document method.
    async fn document_context_for(&self, uri: &Uri) -> Result<DocumentMethodContext> {
        let captured = ingress::document_request()
            .ok_or_else(|| {
                methods::internal_error(anyhow::anyhow!(
                    "document request bypassed ordered LSP ingress"
                ))
            })?
            .map_err(|unavailable| {
                tracing::debug!(
                    path = ?unavailable.path().map(Path::display),
                    reason = unavailable.reason(),
                    "document request has no current synchronized text"
                );
                methods::temporarily_unavailable(unavailable.reason())
            })?;

        if methods::uri_to_path(uri).as_deref() != Some(captured.document().path()) {
            return Err(methods::internal_error(anyhow::anyhow!(
                "captured editor document does not match the LSP request URI"
            )));
        }

        // The document capture is ready, but its engine route may still be starting. Return a
        // temporary result instead of making this query wait. A client retry will take another
        // fresh capture, while the server keeps the synchronized editor text in the meantime.
        let engine_client = captured
            .engine_client()
            .map_err(|reason| methods::temporarily_unavailable(&reason))?;

        tracing::trace!(
            path = %captured.document().path().display(),
            session = captured.document().session().get(),
            revision = captured.document().revision().get(),
            client_version = ?captured.document().client_version(),
            "using document revision captured at LSP ingress"
        );

        Ok(DocumentMethodContext::new(engine_client, captured))
    }

    /// Add the logical request ownership used only by completion.
    async fn completion_context_for(&self, uri: &Uri) -> Result<CompletionMethodContext> {
        let document = self.document_context_for(uri).await?;
        let request = ingress::completion_request().ok_or_else(|| {
            methods::internal_error(anyhow::anyhow!(
                "completion request bypassed ordered LSP ingress"
            ))
        })?;
        let client_capabilities = self
            .client_capabilities
            .get()
            .copied()
            .unwrap_or_default()
            .completion;
        Ok(CompletionMethodContext::new(
            document,
            request,
            client_capabilities,
        ))
    }

    async fn active_engine_client(&self) -> Result<Option<EngineClient>> {
        let Some(engine_client) = self
            .registry()
            .await?
            .active_engine()
            .await
            .map_err(methods::internal_error)?
        else {
            return Ok(None);
        };

        Ok(Some(engine_client))
    }
}

impl LanguageServer for Backend {
    #[tracing::instrument(skip_all, fields(rg.method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let workspace_folders = workspace_folders(&params).map_err(|error| {
            Error::invalid_params(format!("invalid workspace folder: {error:#}"))
        })?;
        if workspace_folders.is_empty() {
            return Err(Error::invalid_params(
                "rust-glancer requires at least one filesystem workspace folder",
            ));
        }

        let config = ServerConfig::from_initialization_options(
            params.initialization_options.as_ref(),
            &workspace_folders,
        )
        .map_err(|error| Error::invalid_params(error.to_string()))?;
        let client_capabilities =
            EngineClientCapabilities::from_lsp_client_capabilities(&params.capabilities);
        let client_status_capabilities =
            ClientStatusCapabilities::from_lsp_client_capabilities(&params.capabilities);
        self.client_capabilities
            .set(client_capabilities)
            .map_err(|_| Error {
                code: ErrorCode::InvalidRequest,
                message: Cow::Borrowed("rust-glancer client capabilities are already initialized"),
                data: None,
            })?;
        let engines = EngineRegistry::new(
            self.lsp_client.clone(),
            workspace_folders.clone(),
            config,
            self.editor.clone(),
            client_status_capabilities,
        );
        let project_watcher = ProjectWatcher::spawn(
            workspace_folders,
            engines.clone(),
            self.recent_editor_saves.clone(),
        );

        self.engines.set(engines).map_err(|_| Error {
            code: ErrorCode::InvalidRequest,
            message: Cow::Borrowed("rust-glancer engine registry is already initialized"),
            data: None,
        })?;
        self.project_watcher
            .set(project_watcher)
            .map_err(|_| Error {
                code: ErrorCode::InvalidRequest,
                message: Cow::Borrowed("rust-glancer project watcher is already initialized"),
                data: None,
            })?;

        Ok(methods::initialize())
    }

    #[tracing::instrument(skip_all, fields(rg.method = "initialized"))]
    async fn initialized(&self, _params: InitializedParams) {
        tracing::debug!("rust-glancer LSP server initialized");
    }

    #[tracing::instrument(skip_all, fields(rg.method = "shutdown"))]
    async fn shutdown(&self) -> Result<()> {
        let Ok(registry) = self.registry().await else {
            return Ok(());
        };

        registry.begin_shutdown().await;
        for engine_client in registry.engine_clients().await {
            if let Err(error) = methods::shutdown(engine_client).await {
                tracing::debug!(error = %error, "failed to shut down rust-glancer engine");
            }
        }

        Ok(())
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "didOpen", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        methods::text_document::did_open::did_open(self.registry().await).await;
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "didChange", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // `EditorIngress` already applied all accepted edits, or marked the synchronized text as
        // unavailable if an edit was rejected, before this async method was allowed to start.
        // There is no remaining operation to place in a separate method handler.
        let _ = params;
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "didSave", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        methods::text_document::did_save::did_save(&self.lsp_client, &self.inlay_refresher, params)
            .await;
    }

    #[tracing::instrument(skip_all, fields(rg.method = "didChangeWatchedFiles"))]
    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        tracing::debug!(
            change_count = params.changes.len(),
            "ignored client watched-file notification; server-side project watcher owns disk changes"
        );
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "didClose", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        methods::text_document::did_close::did_close(self.registry().await).await;
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "gotoDefinition",
            rg.uri = %params.text_document_position_params.text_document.uri.as_str()
        )
    )]
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let context = self
            .document_context_for(&params.text_document_position_params.text_document.uri)
            .await?;
        methods::text_document::definition::definition(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "gotoTypeDefinition",
            rg.uri = %params.text_document_position_params.text_document.uri.as_str()
        )
    )]
    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let context = self
            .document_context_for(&params.text_document_position_params.text_document.uri)
            .await?;
        methods::text_document::type_definition::type_definition(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "gotoImplementation",
            rg.uri = %params.text_document_position_params.text_document.uri.as_str()
        )
    )]
    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        let context = self
            .document_context_for(&params.text_document_position_params.text_document.uri)
            .await?;
        methods::text_document::implementation::implementation(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "references",
            rg.uri = %params.text_document_position.text_document.uri.as_str()
        )
    )]
    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let context = self
            .document_context_for(&params.text_document_position.text_document.uri)
            .await?;
        methods::text_document::references::references(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "prepareRename",
            rg.uri = %params.text_document.uri.as_str()
        )
    )]
    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let context = self.document_context_for(&params.text_document.uri).await?;
        methods::text_document::rename::prepare_rename(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "rename",
            rg.uri = %params.text_document_position.text_document.uri.as_str()
        )
    )]
    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let context = self
            .document_context_for(&params.text_document_position.text_document.uri)
            .await?;
        methods::text_document::rename::rename(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "documentHighlight",
            rg.uri = %params.text_document_position_params.text_document.uri.as_str()
        )
    )]
    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let context = self
            .document_context_for(&params.text_document_position_params.text_document.uri)
            .await?;
        methods::text_document::document_highlight::document_highlight(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "hover",
            rg.uri = %params.text_document_position_params.text_document.uri.as_str()
        )
    )]
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let context = self
            .document_context_for(&params.text_document_position_params.text_document.uri)
            .await?;
        methods::text_document::hover::hover(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "codeAction",
            rg.uri = %params.text_document.uri.as_str()
        )
    )]
    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let client_capabilities = self
            .client_capabilities
            .get()
            .copied()
            .unwrap_or_default()
            .code_action;
        let context = self.document_context_for(&params.text_document.uri).await?;
        methods::text_document::code_action::code_action(context, params, client_capabilities).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            rg.method = "completion",
            rg.uri = %params.text_document_position.text_document.uri.as_str()
        )
    )]
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let context = self
            .completion_context_for(&params.text_document_position.text_document.uri)
            .await?;
        methods::text_document::completion::completion(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "formatting", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let context = self.document_context_for(&params.text_document.uri).await?;
        methods::text_document::formatting::formatting(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "documentSymbol", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let context = self.document_context_for(&params.text_document.uri).await?;
        methods::text_document::document_symbol::document_symbol(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "inlayHint", rg.uri = %params.text_document.uri.as_str())
    )]
    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let context = self.document_context_for(&params.text_document.uri).await?;
        methods::text_document::inlay_hint::inlay_hint(context, params).await
    }

    #[tracing::instrument(skip_all, fields(rg.method = "workspaceSymbol"))]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let Some(engine_client) = self.active_engine_client().await? else {
            return Ok(Some(WorkspaceSymbolResponse::Nested(Vec::new())));
        };
        methods::workspace::symbol::symbol(engine_client, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(rg.method = "executeCommand", rg.command = %params.command)
    )]
    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        let Some(engine_client) = self.active_engine_client().await? else {
            return Err(Error {
                code: ErrorCode::InvalidRequest,
                message: Cow::Borrowed("Rust Glancer has no active Rust project for this command"),
                data: None,
            });
        };
        methods::workspace::execute_command::execute_command(engine_client, params).await
    }
}

fn workspace_folders(params: &InitializeParams) -> anyhow::Result<Vec<NormalizedPathBuf>> {
    let mut folders = params
        .workspace_folders
        .as_ref()
        .into_iter()
        .flatten()
        .map(|folder| {
            let path = rg_lsp_proto::file_uri_to_path(&folder.uri).with_context(|| {
                format!("while converting workspace URI `{}`", folder.uri.as_str())
            })?;
            NormalizedPathBuf::from_absolute(&path)
                .with_context(|| format!("while normalizing workspace path `{}`", path.display()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    folders.sort();
    folders.dedup();
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr};

    use rg_std::NormalizedPathBuf;
    use tower_lsp_server::ls_types::{InitializeParams, Uri, WorkspaceFolder};

    use super::workspace_folders;

    #[test]
    fn workspace_folders_keep_unique_filesystem_roots_in_stable_order() {
        let root = std::env::current_dir()
            .expect("test process should have a current directory")
            .join("server-workspaces");
        let project_a = root.join("project_a");
        let project_b = root.join("project_b");
        let params = InitializeParams {
            workspace_folders: Some(vec![
                workspace_folder(&project_b),
                workspace_folder(&project_a),
                workspace_folder(&project_b),
            ]),
            ..Default::default()
        };

        let project_a =
            NormalizedPathBuf::from_absolute(project_a).expect("project A path should normalize");
        let project_b =
            NormalizedPathBuf::from_absolute(project_b).expect("project B path should normalize");
        assert_eq!(
            workspace_folders(&params).expect("workspace folders should normalize"),
            vec![project_a, project_b],
        );
    }

    #[test]
    fn workspace_folders_reject_non_file_uris() {
        let params = InitializeParams {
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: Uri::from_str("untitled:Scratch").expect("untitled URI should be valid"),
                name: "scratch".to_string(),
            }]),
            ..Default::default()
        };

        let error = workspace_folders(&params)
            .expect_err("non-file workspace folder should be rejected explicitly");
        assert!(error.to_string().contains("converting workspace URI"));
    }

    fn workspace_folder(path: &Path) -> WorkspaceFolder {
        WorkspaceFolder {
            uri: rg_lsp_proto::path_to_file_uri(path).expect("test path should convert to URI"),
            name: path
                .file_name()
                .expect("test path should have a file name")
                .to_string_lossy()
                .into_owned(),
        }
    }
}
