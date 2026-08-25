//! Publication of engine-originated side effects to the concrete LSP client.
//!
//! Progress, refreshes, and logs are direct presentation operations. Saved-source diagnostics need
//! one extra boundary check: the server compares the reported saved text with its authoritative
//! open editor value and keeps prior diagnostics visible when the two revisions differ.

use rg_lsp_proto::{
    EngineError, EngineResult, NotificationsService, ServiceLogLevel, ServiceNotification,
    path_to_file_uri,
};
use rg_std::NormalizedPathBuf;
use tarpc::context;
use tower_lsp_server::{Client as LspClient, ls_types::MessageType};

use crate::{
    client_status::{ClientStatusPublisher, work_done_progress},
    ingress::EditorStateHandle,
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
            // RPC paths are plain transport values. Reestablish their filesystem identity before
            // comparing them with normalized analysis routes.
            let path = match NormalizedPathBuf::from_absolute(&path) {
                Ok(path) => path,
                Err(error) => {
                    tracing::debug!(
                        path = %path.display(),
                        error = %error,
                        "ignored diagnostics with an invalid filesystem path"
                    );
                    return Ok(());
                }
            };
            let publications = editor.diagnostics_publications(&path, saved_text.as_deref());
            if publications.is_empty() {
                tracing::debug!(
                    path = %path.display(),
                    "kept saved-source diagnostics unchanged for newer open document text"
                );
                return Ok(());
            }

            for publication in publications {
                let Ok(uri) = path_to_file_uri(publication.path()) else {
                    tracing::debug!(
                        path = %publication.path().display(),
                        "failed to convert diagnostics path to URI"
                    );
                    continue;
                };
                lsp_client
                    .publish_diagnostics(uri, diagnostics.clone(), publication.version())
                    .await;
            }
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
        ServiceNotification::DeferredIndexingStarted { root, generation } => {
            client_status
                .deferred_indexing_started(&root, generation)
                .await;
        }
        ServiceNotification::DeferredIndexingProgress {
            root,
            generation,
            progress,
        } => {
            client_status
                .deferred_indexing_progress(&root, generation, progress)
                .await;
        }
        ServiceNotification::DeferredIndexingFinished {
            root,
            generation,
            outcome,
        } => {
            client_status
                .deferred_indexing_finished(&root, generation, outcome)
                .await;
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
