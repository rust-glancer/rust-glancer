mod command;
mod worker;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
    time::Instant,
};

use anyhow::Context as _;
use rg_lsp_proto::{ServiceLogLevel, ServiceNotification};
use tokio::sync::{Mutex, oneshot};

pub(crate) use self::command::EngineCommand;
use self::{command::EngineResponse, worker::EngineWorker};
use crate::{
    documents::{DirtyDocumentSnapshotState, DocumentStore},
    memory::MemoryControl,
    service::ServiceNotificationsSink,
};

/// Handle for the long-lived analysis worker.
///
/// The worker itself stays on a dedicated thread because project analysis is mostly synchronous.
/// This handle is the async side used by the RPC-facing service.
#[derive(Clone, Debug)]
pub(crate) struct EngineHandle {
    sender: Sender<QueuedEngineCommand>,
    pub(crate) documents: Arc<Mutex<DocumentStore>>,
    notifications: ServiceNotificationsSink,
}

/// Separates time spent waiting behind older commands from time spent executing this command.
#[derive(Debug)]
pub(crate) struct QueuedEngineCommand {
    pub(crate) command: EngineCommand,
    pub(crate) enqueued_at: Instant,
}

impl QueuedEngineCommand {
    fn new(command: EngineCommand) -> Self {
        Self {
            command,
            enqueued_at: Instant::now(),
        }
    }
}

impl EngineHandle {
    /// Starts the current in-process worker behind the engine-service abstraction.
    pub(crate) fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
        documents: Arc<Mutex<DocumentStore>>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || EngineWorker::new(memory_control).run(receiver));

        Self {
            sender,
            documents,
            notifications,
        }
    }

    pub(crate) async fn request<T>(
        &self,
        build: impl FnOnce(EngineResponse<T>) -> EngineCommand,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        let (respond_to, response) = oneshot::channel();
        self.sender
            .send(QueuedEngineCommand::new(build(respond_to)))
            .context("while attempting to send LSP engine command")?;

        response
            .await
            .context("while attempting to receive LSP engine response")?
    }

    pub(crate) async fn dirty_document_snapshot(&self, path: &Path) -> DirtyDocumentSnapshotState {
        let documents = self.documents.lock().await;
        let freshness = documents.freshness(path);
        let dirty = documents.dirty_snapshot(path);
        drop(documents);

        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "checked document freshness"
        );

        match &dirty {
            DirtyDocumentSnapshotState::Dirty(snapshot) => {
                tracing::debug!(
                    path = %snapshot.path().display(),
                    version = ?snapshot.version(),
                    "using dirty document snapshot for analysis query"
                );
            }
            DirtyDocumentSnapshotState::DirtyWithoutText => {
                tracing::debug!(
                    path = %path.display(),
                    "dirty document has no full-text snapshot"
                );
            }
            DirtyDocumentSnapshotState::Clean => {}
        }

        dirty
    }

    pub(crate) async fn mark_dirty_after_failed_save(&self, path: PathBuf, error: anyhow::Error) {
        let mut documents = self.documents.lock().await;
        documents.mark_dirty_after_failed_save(path.clone());
        let freshness = documents.freshness(&path);
        drop(documents);

        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness after failed save reindex"
        );

        let message = format!("failed to process saved file: {error}");
        self.notifications.send(ServiceNotification::LogMessage {
            level: ServiceLogLevel::Error,
            message,
        });
    }

    pub(crate) async fn log_freshness_after_save(&self, path: &Path) {
        let freshness = self.documents.lock().await.freshness(path);
        tracing::trace!(
            path = %path.display(),
            tracked = freshness.tracked(),
            version = ?freshness.version(),
            dirty = freshness.dirty(),
            saved_len = ?freshness.saved_len(),
            live_len = ?freshness.live_len(),
            saved_hash = ?freshness.saved_hash(),
            live_hash = ?freshness.live_hash(),
            "document freshness after save reindex"
        );
    }

    pub(crate) fn refresh_inlay_hints(&self) {
        self.notifications
            .send(ServiceNotification::InlayHintRefresh);
    }
}
