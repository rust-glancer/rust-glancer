use std::sync::Arc;

use tokio::{sync::Mutex, task::JoinHandle};

use crate::{documents::DocumentStore, events::EngineEventSink};

use super::{
    CheckHandleInner, CheckSnapshot, CurrentCheck,
    command::CargoDiagnosticsCommand,
    progress::{CheckProgress, ProgressFinish},
    publish::WorkspaceDiagnostics,
};

/// Owns the shared handles needed by the spawned diagnostics task.
///
/// Keeping the task body here lets `CheckHandle` stay focused on lifecycle decisions: whether to
/// launch, what to cancel, and which task is currently active.
pub(super) struct CheckTaskContext {
    events: EngineEventSink,
    documents: Arc<Mutex<DocumentStore>>,
    inner: Arc<Mutex<CheckHandleInner>>,
    current: Arc<Mutex<Option<CurrentCheck>>>,
}

impl CheckTaskContext {
    pub(super) fn new(
        events: EngineEventSink,
        documents: Arc<Mutex<DocumentStore>>,
        inner: Arc<Mutex<CheckHandleInner>>,
        current: Arc<Mutex<Option<CurrentCheck>>>,
    ) -> Self {
        Self {
            events,
            documents,
            inner,
            current,
        }
    }

    pub(super) fn spawn(self, snapshot: CheckSnapshot, progress: CheckProgress) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run(snapshot, progress).await;
        })
    }

    async fn run(self, snapshot: CheckSnapshot, progress: CheckProgress) {
        let generation = snapshot.generation;
        let command = snapshot.config.user_facing_command();
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

        workspace_diagnostics.publish(&self.events);

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
