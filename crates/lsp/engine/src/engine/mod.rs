//! Async access to the single-lane LSP analysis engine.
//!
//! RPC handlers clone `EngineHandle` and enqueue typed commands. A dedicated thread consumes those
//! commands in FIFO order, which keeps saved-project mutation, query-time materialization, and
//! package offloading from racing each other. The child modules split that thread into command
//! dispatch, project lifecycle ownership, and request-scoped query execution.

mod command;
mod dispatcher;
mod project;
mod query;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_proto::{ServiceLogLevel, ServiceNotification};
use tokio::sync::{Mutex, oneshot};

pub(crate) use self::{command::EngineCommand, project::ProjectConfiguration};
use self::{command::EngineResponse, dispatcher::EngineDispatcher};
use crate::{
    debounce::Debouncer,
    dirty_state::DirtyState,
    documents::{DirtyDocumentSnapshotState, DocumentStore},
    memory::MemoryControl,
    service::ServiceNotificationsSink,
};

const INLAY_HINT_REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Handle for the long-lived analysis engine.
///
/// The engine itself stays on a dedicated thread because project analysis is mostly synchronous.
/// This handle is the async side used by the RPC-facing service: each call sends one command and
/// awaits its one-shot response without exposing the project itself to async tasks.
#[derive(Clone, Debug)]
pub(crate) struct EngineHandle {
    sender: Sender<QueuedEngineCommand>,
    pub(crate) documents: Arc<Mutex<DocumentStore>>,
    inlay_hint_debouncer: Debouncer,
    notifications: ServiceNotificationsSink,
    dirty_state: DirtyState,
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
    /// Starts the in-process engine behind the service abstraction.
    pub(crate) fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
        documents: Arc<Mutex<DocumentStore>>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let dirty_state = DirtyState::default();
        let inlay_hint_debouncer = Debouncer::new(INLAY_HINT_REFRESH_DEBOUNCE);

        thread::spawn({
            let dirty_state = dirty_state.clone();
            let sender = sender.clone();
            let notifications = notifications.clone();
            move || {
                EngineDispatcher::new(sender, memory_control, dirty_state, notifications)
                    .run(receiver)
            }
        });

        Self {
            sender,
            documents,
            inlay_hint_debouncer,
            notifications,
            dirty_state,
        }
    }

    /// Send one typed command and wait for the dispatcher to answer it.
    ///
    /// Dropping the waiting RPC future closes the response endpoint. Query lifecycle code notices
    /// that before doing expensive analysis, so cancelled requests can remain ordinary queued
    /// commands instead of needing a second cancellation protocol.
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
            .context("send LSP engine command")?;

        response.await.context("receive LSP engine response")?
    }

    /// Clone the query-visible buffer snapshot before the request enters the engine queue.
    ///
    /// The command then carries one stable text/version pair even if the editor sends another
    /// change while it waits. `DirtyState` separately tells the dispatcher when that pair became
    /// stale.
    pub(crate) async fn dirty_document_snapshot(&self, path: &Path) -> DirtyDocumentSnapshotState {
        let documents = self.documents.lock().await;
        let dirty = documents.dirty_snapshot(path);
        drop(documents);

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

    /// Publish the latest lightweight dirty identity to the synchronous engine thread.
    pub(crate) fn sync_dirty_state(&self, path: &Path, dirty: &DirtyDocumentSnapshotState) {
        self.dirty_state.sync_document(path, dirty);
    }

    /// Restore dirty status when disk reindexing failed after `didSave` optimistically cleaned it.
    pub(crate) async fn mark_dirty_after_failed_save(&self, path: PathBuf, error: anyhow::Error) {
        let mut documents = self.documents.lock().await;
        documents.mark_dirty_after_failed_save(path.clone());
        let freshness = documents.freshness(&path);
        let dirty = documents.dirty_snapshot(&path);
        self.sync_dirty_state(&path, &dirty);
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

        let message = format!("failed to process saved file: {error:#}");
        self.notifications.send(ServiceNotification::LogMessage {
            level: ServiceLogLevel::Error,
            message,
        });
    }

    /// Trace the saved/live identity after the project accepted a save reindex.
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

    /// Schedules an inlay-hint refresh after nearby edit notifications settle.
    pub(crate) fn refresh_inlay_hints_debounced(&self) {
        let notifications = self.notifications.clone();
        self.inlay_hint_debouncer.call(move || {
            notifications.send(ServiceNotification::InlayHintRefresh);
        });
    }

    /// Sends an inlay-hint refresh immediately and cancels any pending debounced refresh.
    pub(crate) fn refresh_inlay_hints_now(&self) {
        let notifications = self.notifications.clone();
        self.inlay_hint_debouncer.call_now(move || {
            notifications.send(ServiceNotification::InlayHintRefresh);
        });
    }
}
