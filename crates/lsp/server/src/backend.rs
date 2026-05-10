use tower_lsp_server::{
    Client, LanguageServer,
    jsonrpc::Result,
    ls_types::{request::*, *},
};

use crate::{
    engine_process::EngineProcess,
    methods::{self, MethodContext},
};

#[derive(Debug)]
pub(crate) struct Backend {
    client: Client,
    engine: EngineProcess,
}

impl Backend {
    pub(crate) fn new(client: Client, engine: EngineProcess) -> Self {
        Self { client, engine }
    }

    fn method_context(&self) -> MethodContext<'_> {
        MethodContext {
            lsp_client: &self.client,
            engine_client: self.engine.client(),
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        methods::initialize(self.method_context(), params).await
    }

    async fn initialized(&self, params: InitializedParams) {
        methods::initialized(self.method_context(), params).await;
    }

    async fn shutdown(&self) -> Result<()> {
        // TODO: Once multi-project workspaces are supported, drop the corresponding engine handle.
        methods::shutdown(self.method_context()).await
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        methods::text_document::did_open::did_open(self.method_context(), params).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        methods::text_document::did_change::did_change(self.method_context(), params).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        methods::text_document::did_save::did_save(self.method_context(), params).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        methods::text_document::did_close::did_close(self.method_context(), params).await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        methods::text_document::definition::definition(self.method_context(), params).await
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        methods::text_document::type_definition::type_definition(self.method_context(), params)
            .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        methods::text_document::hover::hover(self.method_context(), params).await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        methods::text_document::completion::completion(self.method_context(), params).await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        methods::text_document::document_symbol::document_symbol(self.method_context(), params)
            .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        methods::text_document::inlay_hint::inlay_hint(self.method_context(), params).await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        methods::workspace::symbol::symbol(self.method_context(), params).await
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        methods::workspace::execute_command::execute_command(self.method_context(), params).await
    }
}
