use tower_lsp_server::{
    Client, LanguageServer,
    jsonrpc::Result,
    ls_types::{MessageType, request::*, *},
};

use crate::{
    engine_client::EngineClient,
    engine_registry::EngineRegistry,
    methods::{self, MethodContext, ServerContext},
};

#[derive(Debug)]
pub(crate) struct Backend {
    client: Client,
    engines: EngineRegistry,
}

impl Backend {
    pub(crate) fn new(client: Client) -> Self {
        let engines = EngineRegistry::new(client.clone());
        Self { client, engines }
    }

    fn server_context(&self) -> ServerContext<'_> {
        ServerContext {
            engines: &self.engines,
        }
    }

    fn method_context(&self, engine_client: EngineClient) -> MethodContext {
        MethodContext { engine_client }
    }

    async fn method_context_for(&self, uri: &Uri) -> anyhow::Result<Option<MethodContext>> {
        let Some(path) = methods::uri_to_path(uri) else {
            return Ok(None);
        };
        let Some(engine_client) = self
            .engines
            .engine_for_document(&path)
            .await
            .inspect_err(|_| tracing::error!("failed to route LSP method to an engine"))?
        else {
            return Ok(None);
        };

        Ok(Some(self.method_context(engine_client)))
    }

    async fn active_method_context(&self) -> Option<MethodContext> {
        self.engines
            .active_engine()
            .await
            .map(|engine_client| self.method_context(engine_client))
    }
}

impl LanguageServer for Backend {
    #[tracing::instrument(skip_all, fields(method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        methods::initialize(self.server_context(), params).await
    }

    #[tracing::instrument(skip_all, fields(method = "initialized"))]
    async fn initialized(&self, params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "rust-glancer initialized")
            .await;

        for engine in self.engines.uninitialized_engines().await {
            let context = self.method_context(engine.client);
            if let Err(error) = methods::initialized(context, params).await {
                tracing::error!(
                    root = %engine.root.display(),
                    error = %error,
                    "failed to initialize rust-glancer engine"
                );
                continue;
            }

            self.engines.mark_initialized(&engine.root).await;
        }
    }

    #[tracing::instrument(skip_all, fields(method = "shutdown"))]
    async fn shutdown(&self) -> Result<()> {
        for engine_client in self.engines.engine_clients().await {
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
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_open::did_open(context, params).await;
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
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        methods::text_document::did_close::did_close(context, params).await;
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
            .await
            .map_err(methods::internal_error)?
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
            .await
            .map_err(methods::internal_error)?
        else {
            return Ok(None);
        };
        methods::text_document::type_definition::type_definition(context, params).await
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
            .await
            .map_err(methods::internal_error)?
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
            .await
            .map_err(methods::internal_error)?
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
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .map_err(methods::internal_error)?
        else {
            return Ok(None);
        };
        methods::text_document::document_symbol::document_symbol(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "inlayHint", uri = ?params.text_document.uri)
    )]
    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let Some(context) = self
            .method_context_for(&params.text_document.uri)
            .await
            .map_err(methods::internal_error)?
        else {
            return Ok(None);
        };
        methods::text_document::inlay_hint::inlay_hint(context, params).await
    }

    #[tracing::instrument(skip_all, fields(method = "workspaceSymbol"))]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let Some(context) = self.active_method_context().await else {
            return Ok(Some(WorkspaceSymbolResponse::Nested(Vec::new())));
        };
        methods::workspace::symbol::symbol(context, params).await
    }

    #[tracing::instrument(
        skip_all,
        fields(method = "executeCommand", command = %params.command)
    )]
    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        let Some(context) = self.active_method_context().await else {
            return Ok(None);
        };
        methods::workspace::execute_command::execute_command(context, params).await
    }
}
