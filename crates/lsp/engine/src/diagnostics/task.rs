use std::sync::Arc;

use tokio::{sync::Mutex, task::JoinHandle};

use crate::{documents::DocumentStore, service::ServiceNotificationsSink};

use super::{
    CurrentDiagnostics, DiagnosticsHandleInner, DiagnosticsSnapshot,
    command::CargoDiagnosticsCommand,
    progress::{DiagnosticsProgress, ProgressFinish},
    publish::WorkspaceDiagnostics,
};

/// Owns the shared handles needed by the spawned diagnostics task.
///
/// Keeping the task body here lets `DiagnosticsHandle` stay focused on lifecycle decisions: whether to
/// launch, what to cancel, and which task is currently active.
pub(super) struct DiagnosticsTaskContext {
    notifications: ServiceNotificationsSink,
    documents: Arc<Mutex<DocumentStore>>,
    inner: Arc<Mutex<DiagnosticsHandleInner>>,
    current: Arc<Mutex<Option<CurrentDiagnostics>>>,
}

impl DiagnosticsTaskContext {
    pub(super) fn new(
        notifications: ServiceNotificationsSink,
        documents: Arc<Mutex<DocumentStore>>,
        inner: Arc<Mutex<DiagnosticsHandleInner>>,
        current: Arc<Mutex<Option<CurrentDiagnostics>>>,
    ) -> Self {
        Self {
            notifications,
            documents,
            inner,
            current,
        }
    }

    pub(super) fn spawn(
        self,
        snapshot: DiagnosticsSnapshot,
        progress: DiagnosticsProgress,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run(snapshot, progress).await;
        })
    }

    async fn run(self, snapshot: DiagnosticsSnapshot, progress: DiagnosticsProgress) {
        let generation = snapshot.generation;
        let command = snapshot.config.user_facing_command(&snapshot.analysis);
        progress.begin(command).await;

        let result = CargoDiagnosticsCommand::new(snapshot).run().await;
        let workspace_diagnostics = match result {
            Ok(diagnostics) => {
                let mut inner = self.inner.lock().await;
                if inner.generation != generation {
                    progress.finish(ProgressFinish::Superseded).await;
                    return;
                }

                let documents = self.documents.lock().await;
                let mut workspace_diagnostics =
                    WorkspaceDiagnostics::new(diagnostics, &documents, &inner.published_paths);
                inner.published_paths = workspace_diagnostics.take_published_paths();
                workspace_diagnostics
            }
            Err(error) => {
                tracing::error!(
                    generation,
                    error = %error,
                    "cargo diagnostics run failed"
                );
                self.current.lock().await.take();
                progress.finish(ProgressFinish::Failed).await;
                return;
            }
        };

        workspace_diagnostics.publish(&self.notifications);

        let mut current = self.current.lock().await;
        if current
            .as_ref()
            .is_some_and(|current| current.progress.token() == progress.token())
        {
            current.take();
        }
        progress.finish(ProgressFinish::Finished).await;
    }
}
