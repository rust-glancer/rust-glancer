use std::{borrow::Cow, path::PathBuf};

use tower_lsp_server::{
    Client as LspClient, LanguageServer,
    jsonrpc::{Error, ErrorCode, Result},
    ls_types::{request::*, *},
};

use rg_lsp_proto::EngineConfig;
use tokio::sync::OnceCell;

use crate::{
    engine_client::EngineClient,
    engine_registry::EngineRegistry,
    methods::{self, MethodContext},
};

#[derive(Debug)]
pub(crate) struct Backend {
    lsp_client: LspClient,
    engines: OnceCell<EngineRegistry>,
}

impl Backend {
    pub(crate) fn new(lsp_client: LspClient) -> Self {
        Self {
            lsp_client,
            engines: OnceCell::new(),
        }
    }

    async fn registry(&self) -> Result<&EngineRegistry> {
        self.engines.get().ok_or(Error {
            code: ErrorCode::ServerError(-32002),
            message: Cow::Borrowed("rust-glancer engine registry is not initialized"),
            data: None,
        })
    }

    fn method_context(&self, engine_client: EngineClient) -> MethodContext {
        MethodContext { engine_client }
    }

    async fn method_context_for(&self, uri: &Uri) -> Result<Option<MethodContext>> {
        let Some(path) = methods::uri_to_path(uri) else {
            return Ok(None);
        };
        let Some(engine_client) = self
            .registry()
            .await?
            .document(&path)
            .await
            .inspect_err(|_| tracing::error!("failed to route LSP method to an engine"))
            .map_err(methods::internal_error)?
        else {
            return Ok(None);
        };

        Ok(Some(self.method_context(engine_client)))
    }

    async fn active_method_context(&self) -> Result<Option<MethodContext>> {
        Ok(self
            .registry()
            .await?
            .active_engine()
            .await
            .map(|engine_client| self.method_context(engine_client)))
    }
}

impl LanguageServer for Backend {
    #[tracing::instrument(skip_all, fields(method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let workspace_folders = workspace_folders(&params);
        if workspace_folders.is_empty() {
            return Err(Error::invalid_params(
                "rust-glancer requires at least one filesystem workspace folder",
            ));
        }

        let config =
            EngineConfig::from_initialization_options(params.initialization_options.as_ref())
                .map_err(|error| Error::invalid_params(error.to_string()))?;
        let engines = EngineRegistry::new(self.lsp_client.clone(), workspace_folders, config);

        self.engines.set(engines).map_err(|_| Error {
            code: ErrorCode::InvalidRequest,
            message: Cow::Borrowed("rust-glancer engine registry is already initialized"),
            data: None,
        })?;

        Ok(methods::initialize())
    }

    #[tracing::instrument(skip_all, fields(method = "initialized"))]
    async fn initialized(&self, _params: InitializedParams) {
        tracing::debug!("rust-glancer LSP server initialized");
    }

    #[tracing::instrument(skip_all, fields(method = "shutdown"))]
    async fn shutdown(&self) -> Result<()> {
        let Ok(registry) = self.registry().await else {
            return Ok(());
        };

        for engine_client in registry.engine_clients().await {
            let context = self.method_context(engine_client);
            if let Err(error) = methods::shutdown(context).await {
                tracing::debug!(error = %error, "failed to shut down rust-glancer engine");
            }
        }

        Ok(())
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "didOpen", uri = ?params.text_document.uri)
    )]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
            return;
        };
        let Some(registry) = self.registry().await.ok() else {
            return;
        };
        let Some(engine_client) = registry
            .open_document(&path)
            .await
            .inspect_err(|_| tracing::error!("failed to route opened document to an engine"))
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_open::did_open(self.method_context(engine_client), params)
            .await;
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "didChange", uri = ?params.text_document.uri)
    )]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_change::did_change(context, params).await;
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "didSave", uri = ?params.text_document.uri)
    )]
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_save::did_save(context, params).await;
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "didClose", uri = ?params.text_document.uri)
    )]
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
            return;
        };
        let Some(registry) = self.registry().await.ok() else {
            return;
        };
        let Some(engine_client) = registry
            .close_document(&path)
            .await
            .inspect_err(|_| tracing::error!("failed to route closed document to an engine"))
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_close::did_close(self.method_context(engine_client), params)
            .await;
    }

    #[tracing::instrument(
        skip_all,
        fields(
            method = "gotoDefinition",
            uri = ?params.text_document_position_params.text_document.uri
        )
    )]
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let Some(context) = self
            .method_context_for(&params.text_document_position_params.text_document.uri)
            .await?
        else {
            return Ok(None);
        };
        methods::text_document::definition::definition(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            method = "gotoTypeDefinition",
            uri = ?params.text_document_position_params.text_document.uri
        )
    )]
    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let Some(context) = self
            .method_context_for(&params.text_document_position_params.text_document.uri)
            .await?
        else {
            return Ok(None);
        };
        methods::text_document::type_definition::type_definition(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            method = "gotoImplementation",
            uri = ?params.text_document_position_params.text_document.uri
        )
    )]
    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        let Some(context) = self
            .method_context_for(&params.text_document_position_params.text_document.uri)
            .await?
        else {
            return Ok(None);
        };
        methods::text_document::implementation::implementation(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            method = "hover",
            uri = ?params.text_document_position_params.text_document.uri
        )
    )]
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let Some(context) = self
            .method_context_for(&params.text_document_position_params.text_document.uri)
            .await?
        else {
            return Ok(None);
        };
        methods::text_document::hover::hover(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(
            method = "completion",
            uri = ?params.text_document_position.text_document.uri
        )
    )]
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let Some(context) = self
            .method_context_for(&params.text_document_position.text_document.uri)
            .await?
        else {
            return Ok(None);
        };
        methods::text_document::completion::completion(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "documentSymbol", uri = ?params.text_document.uri)
    )]
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let Some(context) = self.method_context_for(&params.text_document.uri).await? else {
            return Ok(None);
        };
        methods::text_document::document_symbol::document_symbol(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "inlayHint", uri = ?params.text_document.uri)
    )]
    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let Some(context) = self.method_context_for(&params.text_document.uri).await? else {
            return Ok(None);
        };
        methods::text_document::inlay_hint::inlay_hint(context, params).await
    }

    #[tracing::instrument(skip_all, fields(method = "workspaceSymbol"))]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let Some(context) = self.active_method_context().await? else {
            return Ok(Some(WorkspaceSymbolResponse::Nested(Vec::new())));
        };
        methods::workspace::symbol::symbol(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "executeCommand", command = %params.command)
    )]
    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        let Some(context) = self.active_method_context().await? else {
            return Err(Error {
                code: ErrorCode::InvalidRequest,
                message: Cow::Borrowed("Rust Glancer has no active Rust project for this command"),
                data: None,
            });
        };
        methods::workspace::execute_command::execute_command(context, params).await
    }
}

fn workspace_folders(params: &InitializeParams) -> Vec<PathBuf> {
    let mut folders = params
        .workspace_folders
        .as_ref()
        .into_iter()
        .flatten()
        .filter_map(|folder| methods::uri_to_path(&folder.uri))
        .collect::<Vec<_>>();
    folders.sort();
    folders.dedup();
    folders
}
