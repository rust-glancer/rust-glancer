use std::sync::Arc;

use rg_lsp_engine::{EngineEventReceiver, EngineEventSink, InProcessEngineService, MemoryControl};
use rg_lsp_proto::{EngineEvent, EngineLogLevel, EngineServiceHandle};
use tower_lsp_server::{
    Client, LanguageServer,
    jsonrpc::Result,
    ls_types::{
        ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
        WorkDoneProgressEnd, notification::Progress, request::*, *,
    },
};

use crate::methods;

#[derive(Debug)]
pub(crate) struct Backend {
    ctx: ServerContext,
}

impl Backend {
    pub(crate) fn new(client: Client, memory_control: Arc<dyn MemoryControl>) -> Self {
        let (events, event_receiver) = EngineEventSink::channel();
        tokio::spawn(forward_engine_events(client.clone(), event_receiver));
        let engine = InProcessEngineService::spawn(memory_control, events);

        Self {
            ctx: ServerContext { client, engine },
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ServerContext {
    pub(crate) client: Client,
    pub(crate) engine: EngineServiceHandle,
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        methods::initialize(&self.ctx, params).await
    }

    async fn initialized(&self, params: InitializedParams) {
        methods::initialized(&self.ctx, params).await;
    }

    async fn shutdown(&self) -> Result<()> {
        methods::shutdown(&self.ctx).await
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        methods::text_document::did_open::did_open(&self.ctx, params).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        methods::text_document::did_change::did_change(&self.ctx, params).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        methods::text_document::did_save::did_save(&self.ctx, params).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        methods::text_document::did_close::did_close(&self.ctx, params).await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        methods::text_document::definition::definition(&self.ctx, params).await
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        methods::text_document::type_definition::type_definition(&self.ctx, params).await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        methods::text_document::hover::hover(&self.ctx, params).await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        methods::text_document::completion::completion(&self.ctx, params).await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        methods::text_document::document_symbol::document_symbol(&self.ctx, params).await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        methods::text_document::inlay_hint::inlay_hint(&self.ctx, params).await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        methods::workspace::symbol::symbol(&self.ctx, params).await
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        methods::workspace::execute_command::execute_command(&self.ctx, params).await
    }
}

/// Publishes engine side effects to the real LSP client.
///
/// The engine deliberately only sends queued events. This task is the orchestration boundary where
/// those events become LSP progress, diagnostics, refreshes, or log messages.
async fn forward_engine_events(client: Client, mut events: EngineEventReceiver) {
    while let Some(event) = events.recv().await {
        match event {
            EngineEvent::PublishDiagnostics {
                path,
                diagnostics,
                version,
            } => {
                let Some(uri) = Uri::from_file_path(&path) else {
                    tracing::debug!(
                        path = %path.display(),
                        "failed to convert diagnostics path to URI"
                    );
                    continue;
                };
                client.publish_diagnostics(uri, diagnostics, version).await;
            }
            EngineEvent::BeginWorkDoneProgress {
                token,
                title,
                message,
            } => {
                if let Err(error) = client.create_work_done_progress(token.clone()).await {
                    tracing::debug!(
                        error = %error,
                        "failed to create engine progress token"
                    );
                    continue;
                }

                let progress = WorkDoneProgressBegin {
                    title,
                    cancellable: Some(false),
                    message,
                    percentage: None,
                };
                client
                    .send_notification::<Progress>(ProgressParams {
                        token,
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(progress)),
                    })
                    .await;
            }
            EngineEvent::EndWorkDoneProgress { token, message } => {
                client
                    .send_notification::<Progress>(ProgressParams {
                        token,
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                            WorkDoneProgressEnd { message },
                        )),
                    })
                    .await;
            }
            EngineEvent::InlayHintRefresh => {
                if let Err(error) = client.inlay_hint_refresh().await {
                    tracing::debug!(
                        error = %error,
                        "failed to request inlay hint refresh after engine event"
                    );
                }
            }
            EngineEvent::LogMessage { level, message } => {
                client.log_message(message_type(level), message).await;
            }
        }
    }
}

fn message_type(level: EngineLogLevel) -> MessageType {
    match level {
        EngineLogLevel::Error => MessageType::ERROR,
        EngineLogLevel::Warning => MessageType::WARNING,
        EngineLogLevel::Info => MessageType::INFO,
        EngineLogLevel::Log => MessageType::LOG,
    }
}
