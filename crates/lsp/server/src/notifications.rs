//! Publication of engine-originated side effects to the concrete LSP client.
//!
//! Progress, refreshes, and logs are direct presentation operations. Saved-source diagnostics need
//! one extra boundary check: the server compares the reported saved text with its authoritative
//! open editor value and keeps prior diagnostics visible when the two revisions differ.

use rg_lsp_proto::{
    EngineError, EngineResult, NotificationsService, ServiceLogLevel, ServiceNotification,
};
use tarpc::context;
use tower_lsp_server::{
    Client as LspClient,
    ls_types::{MessageType, Uri},
};

use crate::{
    client_status::{ClientStatusPublisher, work_done_progress},
    ingress::{DiagnosticsPublication, EditorStateHandle},
};

/// Publishes service side effects to the real LSP client.
///
/// The worker process deliberately only sends protocol-level notifications. This service is the
/// orchestration boundary where those notifications become LSP progress, diagnostics, refreshes, or
/// log messages.
#[derive(Clone, Debug)]
pub(crate) struct NotificationsPublisher {
    lsp_client: LspClient,
    client_status: ClientStatusPublisher,
    editor: EditorStateHandle,
}

impl NotificationsPublisher {
    pub(crate) fn new(
        lsp_client: LspClient,
        editor: EditorStateHandle,
        client_status: ClientStatusPublisher,
    ) -> Self {
        Self {
            lsp_client,
            client_status,
            editor,
        }
    }
}

impl NotificationsService for NotificationsPublisher {
    async fn publish(
        self,
        _: context::Context,
        notification: ServiceNotification,
    ) -> EngineResult<()> {
        publish_service_notification(
            &self.lsp_client,
            &self.client_status,
            &self.editor,
            notification,
        )
        .await
        .map_err(EngineError::from)
    }
}

async fn publish_service_notification(
    lsp_client: &LspClient,
    client_status: &ClientStatusPublisher,
    editor: &EditorStateHandle,
    notification: ServiceNotification,
) -> anyhow::Result<()> {
    match notification {
        ServiceNotification::PublishDiagnostics {
            path,
            diagnostics,
            saved_text,
        } => {
            let DiagnosticsPublication::Publish { version } =
                editor.diagnostics_publication(&path, saved_text.as_deref())
            else {
                tracing::debug!(
                    path = %path.display(),
                    "kept saved-source diagnostics unchanged for newer open document text"
                );
                return Ok(());
            };
            let Some(uri) = Uri::from_file_path(&path) else {
                tracing::debug!(
                    path = %path.display(),
                    "failed to convert diagnostics path to URI"
                );
                return Ok(());
            };
            lsp_client
                .publish_diagnostics(uri, diagnostics, version)
                .await;
        }
        ServiceNotification::BeginWorkDoneProgress {
            token,
            title,
            message,
        } => {
            work_done_progress::begin_engine_progress(lsp_client, token, title, message).await;
        }
        ServiceNotification::EndWorkDoneProgress { token, message } => {
            work_done_progress::end_engine_progress(lsp_client, token, message).await;
        }
        ServiceNotification::InlayHintRefresh => {
            if let Err(error) = lsp_client.inlay_hint_refresh().await {
                tracing::debug!(
                    error = %error,
                    "failed to request inlay hint refresh after service notification"
                );
            }
        }
        ServiceNotification::DeferredIndexingStarted { root } => {
            client_status.deferred_indexing_started(&root).await;
        }
        ServiceNotification::DeferredIndexingFinished { root } => {
            client_status.deferred_indexing_finished(&root).await;
        }
        ServiceNotification::LogMessage { level, message } => {
            lsp_client.log_message(message_type(level), message).await;
        }
    }

    Ok(())
}

fn message_type(level: ServiceLogLevel) -> MessageType {
    match level {
        ServiceLogLevel::Error => MessageType::ERROR,
        ServiceLogLevel::Warning => MessageType::WARNING,
        ServiceLogLevel::Info => MessageType::INFO,
        ServiceLogLevel::Log => MessageType::LOG,
    }
}
