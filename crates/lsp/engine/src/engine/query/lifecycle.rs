//! Work that every analysis request does before and after its feature-specific query.
//!
//! `respond_to_query` owns the common flow:
//!
//! 1. Skip work if the caller has already dropped the response channel, or if the saved project is
//!    already known to be stale.
//! 2. Run the feature query and let expensive phases check whether the response is still wanted.
//! 3. Tag a successful value with the project and document ids used to compute it.
//! 4. Release data loaded only for this request and repair a recoverable package-cache failure.
//!
//! This layer does not decide whether an editor document is still current. Only server ingress
//! owns live sessions and revisions, so the engine returns the captured ids and the server checks
//! them before publishing the value.
//!
//! A saved-source race follows a different path. If hover discovers that `src/lib.rs` changed on
//! disk, hover returns `AnalysisAbort::SourceChanged`, records that the saved generation is stale,
//! and enqueues `src/lib.rs` on the normal path-change stream. Later queries return the same abort
//! until that mutation publishes a new saved project; the hover itself never turns into a
//! synchronous reindex.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use rg_lsp_proto::{
    AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady, EditorDocumentSnapshot,
    GlobalPositionSnapshot, OpenDocumentsRevision, TargetDocumentRevision,
};
use rg_project::Project;

use super::QueryRunner;
use crate::{engine::command::AnalysisResponse, memory::MemoryReporter};

/// Lets expensive query phases check whether anyone still wants the result.
///
/// Dropping the RPC future closes the engine response channel. Long-running queries use this value
/// to check that channel between expensive phases. It carries no document ids and keeps no second
/// cancellation flag that could disagree with the channel.
pub(crate) struct QueryCancellation<'a> {
    is_cancelled: &'a dyn Fn() -> bool,
}

impl<'a> QueryCancellation<'a> {
    fn new(is_cancelled: &'a dyn Fn() -> bool) -> Self {
        Self { is_cancelled }
    }

    /// Stop at a named query boundary if nobody can receive the result anymore.
    pub(crate) fn checkpoint(&self, checkpoint: &'static str) -> anyhow::Result<()> {
        if (self.is_cancelled)() {
            return Err(QueryCancelled { checkpoint }.into());
        }
        Ok(())
    }
}

/// Internal early exit caught by the query lifecycle before it can become a feature error.
#[derive(Debug)]
struct QueryCancelled {
    checkpoint: &'static str,
}

impl fmt::Display for QueryCancelled {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "query cancelled at {}", self.checkpoint)
    }
}

impl std::error::Error for QueryCancelled {}

/// Common request data recorded before one command starts analysis.
///
/// Queue time is kept separate from execution time. The input ids are copied from the command so a
/// successful result can return them to the server for its final stale-result check.
#[derive(Debug)]
pub(crate) struct QueryContext {
    label: &'static str,
    queue_elapsed: Duration,
    input: QueryInputIdentity,
}

/// Which editor ids, if any, must be returned with a successful result.
#[derive(Debug)]
enum QueryInputIdentity {
    SavedProject,
    GlobalOperation {
        target: TargetDocumentRevision,
        open_documents_revision: OpenDocumentsRevision,
    },
    TargetDocument(TargetDocumentRevision),
}

impl QueryContext {
    pub(crate) fn new(label: &'static str, queue_elapsed: Duration) -> Self {
        Self {
            label,
            queue_elapsed,
            input: QueryInputIdentity::SavedProject,
        }
    }

    pub(crate) fn global_operation(
        label: &'static str,
        queue_elapsed: Duration,
        snapshot: &GlobalPositionSnapshot,
    ) -> Self {
        Self {
            label,
            queue_elapsed,
            input: QueryInputIdentity::GlobalOperation {
                target: snapshot.target().clone(),
                open_documents_revision: snapshot.open_documents_revision(),
            },
        }
    }

    /// Record a query that depends on its target document but not on open sibling documents.
    pub(crate) fn target_document(
        label: &'static str,
        queue_elapsed: Duration,
        document: &EditorDocumentSnapshot,
    ) -> Self {
        Self {
            label,
            queue_elapsed,
            input: QueryInputIdentity::TargetDocument(document.target().clone()),
        }
    }

    fn analysis_input(&self, saved_project_generation: u64) -> AnalysisInput {
        match &self.input {
            QueryInputIdentity::SavedProject => {
                AnalysisInput::for_saved_project(saved_project_generation)
            }
            QueryInputIdentity::GlobalOperation {
                target,
                open_documents_revision,
            } => AnalysisInput::for_global_operation(
                saved_project_generation,
                *open_documents_revision,
                target.clone(),
            ),
            QueryInputIdentity::TargetDocument(target) => {
                AnalysisInput::for_target_document(saved_project_generation, target.clone())
            }
        }
    }
}

impl QueryRunner<'_> {
    /// Run one read-only request through the common query lifecycle.
    ///
    /// The closure contains only the feature query and receives a cancellation check it can use
    /// between expensive phases. If reading saved source proves that the project is stale, this
    /// method schedules the normal path-change recovery instead of rebuilding inside the request.
    ///
    /// A finished response is sent before cleanup and recovery start. Cleanup still completes
    /// before the dispatcher accepts the next command, but it does not add to the latency observed
    /// by this request's caller.
    pub(crate) fn respond_to_query<T>(
        &mut self,
        context: QueryContext,
        respond_to: AnalysisResponse<T>,
        query: impl FnOnce(&mut Self, &QueryCancellation<'_>) -> anyhow::Result<T>,
    ) where
        T: Send + 'static,
    {
        // LSP cancellation drops the RPC handler waiting on this response. The command may still
        // be in the dispatcher queue, but there is no reason to materialize packages or run
        // analysis once nobody can receive the result.
        if respond_to.is_closed() {
            tracing::debug!(
                label = context.label,
                queued_ms = context.queue_elapsed.as_millis(),
                "cancelled analysis query skipped"
            );
            return;
        }

        // Once one query proves that the saved generation no longer describes disk, all later
        // queries are known to be unsafe. They remain cheap explicit aborts until the queued
        // watcher/recovery mutation publishes a coherent generation.
        if let Some(stale_source) = self.project.stale_source() {
            tracing::debug!(
                label = context.label,
                path = %stale_source.display(),
                queued_ms = context.queue_elapsed.as_millis(),
                "analysis query skipped for stale saved generation"
            );
            let _ = respond_to.send(Ok(AnalysisOutcome::Aborted(AnalysisAbort::SourceChanged)));
            return;
        }

        // Keeping the stale check in the context layer lets timing and cache recovery remain
        // uniform for every analysis query.
        let label = context.label;
        let queue_elapsed = context.queue_elapsed;
        tracing::trace!(
            label,
            queued_ms = queue_elapsed.as_millis(),
            "analysis query started"
        );
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let memory_before = MemoryReporter::snapshot(memory_control.as_ref());
        let analysis_input = context.analysis_input(self.project.generation());
        let result = {
            let is_cancelled = || respond_to.is_closed();
            query(self, &QueryCancellation::new(&is_cancelled))
        };
        let cancelled_checkpoint = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<QueryCancelled>())
            .map(|cancelled| cancelled.checkpoint);
        let stale_path = if cancelled_checkpoint.is_some() {
            None
        } else {
            result
                .as_ref()
                .err()
                .and_then(Project::stale_source_path)
                .map(std::path::Path::to_path_buf)
        };
        let stale_source_aborted = stale_path.is_some();
        if let Some(stale_path) = &stale_path {
            // A source race is a project-lifecycle event, not a feature-query failure. Re-enter the
            // FIFO mutation stream and abort this response while that recovery catches up.
            self.project.record_stale_source(label, stale_path);
        }
        let query_elapsed = started.elapsed();
        let should_recover = cancelled_checkpoint.is_none()
            && result
                .as_ref()
                .err()
                .is_some_and(Project::is_recoverable_cache_load_failure);
        if let Some(checkpoint) = cancelled_checkpoint {
            tracing::debug!(
                query = label,
                queued_ms = queue_elapsed.as_millis(),
                elapsed_ms = query_elapsed.as_millis(),
                checkpoint,
                "cancelled analysis query stopped"
            );
        } else if stale_source_aborted {
            tracing::info!(
                query = label,
                queued_ms = queue_elapsed.as_millis(),
                elapsed_ms = query_elapsed.as_millis(),
                status = "aborted",
                abort = ?AnalysisAbort::SourceChanged,
                "analysis query completed"
            );
        } else {
            match &result {
                Ok(_) => {
                    tracing::info!(
                        query = label,
                        queued_ms = queue_elapsed.as_millis(),
                        elapsed_ms = query_elapsed.as_millis(),
                        status = "ok",
                        "analysis query completed"
                    );
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    tracing::warn!(
                        query = label,
                        queued_ms = queue_elapsed.as_millis(),
                        elapsed_ms = query_elapsed.as_millis(),
                        status = "error",
                        recoverable_cache_failure = should_recover,
                        error = %error,
                        "analysis query completed"
                    );
                }
            }
        }

        if cancelled_checkpoint.is_some() {
            // The receiver is already gone. Cancellation is an execution detail, not a semantic
            // abort or an empty feature result, so there is deliberately nothing to publish.
        } else if stale_source_aborted {
            let _ = respond_to.send(Ok(AnalysisOutcome::Aborted(AnalysisAbort::SourceChanged)));
        } else if should_recover {
            // Lazy package loads can fail when an offloaded artifact becomes stale between
            // indexing and a query. The next command sees a repaired project; this request remains
            // explicitly unavailable instead of pretending the feature found no result.
            let _ = respond_to.send(Ok(AnalysisOutcome::Aborted(
                AnalysisAbort::TemporarilyUnavailable,
            )));
        } else {
            let _ = respond_to
                .send(result.map(|value| {
                    AnalysisOutcome::Ready(AnalysisReady::new(value, analysis_input))
                }));
        }

        // Publication wakes the RPC task immediately. Request-owned loads and allocator pages are
        // still released synchronously before this dispatcher accepts another command, but that
        // housekeeping no longer delays a result that is already complete and safe to publish.
        self.project.release_query_memory();
        MemoryReporter::purge_and_report_delta_debug(memory_control.as_ref(), label, memory_before);
        if should_recover {
            self.project.recover_after_package_cache_failure(label);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        path::PathBuf,
        sync::{Arc, mpsc},
        time::Duration,
    };

    use anyhow::Context as _;
    use rg_lsp_proto::{
        AnalysisOutcome, DocumentRevision, EditorDocumentSnapshot, GlobalPositionSnapshot,
        OpenDocumentSession, OpenDocumentsRevision, ServiceNotification,
    };
    use tokio::sync::oneshot;

    use super::QueryContext;
    use crate::{
        engine::{project::ProjectCoordinator, query::QueryRunner},
        memory::MemoryControl,
        service::{ServiceNotificationPublisher, ServiceNotificationsSink},
    };

    #[derive(Debug)]
    struct NoopNotifications;

    impl ServiceNotificationPublisher for NoopNotifications {
        fn send(&self, _notification: ServiceNotification) {}
    }

    #[test]
    fn cancelled_query_does_not_run_analysis() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) =
            oneshot::channel::<anyhow::Result<AnalysisOutcome<Vec<usize>>>>();
        drop(response);
        let query_ran = Cell::new(false);

        runner.respond_to_query(
            QueryContext::new("workspace_symbol", Duration::ZERO),
            respond_to,
            |_, _| {
                query_ran.set(true);
                Ok(vec![1])
            },
        );

        assert!(!query_ran.get(), "cancelled query should not run analysis");
    }

    #[test]
    fn global_operation_returns_the_exact_open_document_identity_used_by_analysis() {
        let document = EditorDocumentSnapshot::new(
            PathBuf::from("/workspace/src/lib.rs"),
            OpenDocumentSession::new(3),
            DocumentRevision::new(8),
            Some(5),
            "fn editor() {}".to_string(),
        );
        let snapshot = GlobalPositionSnapshot::new(
            document.target().clone(),
            OpenDocumentsRevision::new(13),
            vec![document],
            ls_types::Position::new(0, 3),
        );
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) = oneshot::channel();
        let context = QueryContext::global_operation("references", Duration::ZERO, &snapshot);

        runner.respond_to_query(context, respond_to, |_, _| Ok(vec![1_usize]));

        let result = futures::executor::block_on(response)
            .expect("document query should send a response")
            .expect("document query should succeed");
        let AnalysisOutcome::Ready(ready) = result else {
            panic!("document query should be ready");
        };
        assert_eq!(ready.value(), &vec![1]);
        assert_eq!(ready.input().target_document(), Some(snapshot.target()));
        assert_eq!(
            ready.input().open_documents_revision(),
            Some(snapshot.open_documents_revision())
        );
    }

    #[test]
    fn valid_empty_query_remains_a_ready_semantic_result() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) = oneshot::channel();

        runner.respond_to_query(
            QueryContext::new("workspace_symbol", Duration::ZERO),
            respond_to,
            |_, _| Ok(Vec::<usize>::new()),
        );

        let result = futures::executor::block_on(response)
            .expect("valid empty query should send a response")
            .expect("valid empty query should succeed");
        let AnalysisOutcome::Ready(ready) = result else {
            panic!("valid empty query should remain ready");
        };
        assert!(ready.value().is_empty());
        assert!(ready.input().target_document().is_none());
    }

    #[test]
    fn query_stops_when_response_closes_during_analysis() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) =
            oneshot::channel::<anyhow::Result<AnalysisOutcome<Vec<usize>>>>();
        let work_after_checkpoint_ran = Cell::new(false);

        runner.respond_to_query(
            QueryContext::new("completion", Duration::ZERO),
            respond_to,
            |_, cancellation| {
                // Model the RPC task disappearing after the engine has already entered the query.
                drop(response);
                cancellation
                    .checkpoint("test semantic work")
                    .context("stop test query after response closure")?;
                work_after_checkpoint_ran.set(true);
                Ok(vec![1])
            },
        );

        assert!(
            !work_after_checkpoint_ran.get(),
            "work after a closed-response checkpoint should not run",
        );
    }

    fn test_project(memory_control: Arc<dyn MemoryControl>) -> ProjectCoordinator {
        let (sender, _receiver) = mpsc::channel();
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        ProjectCoordinator::new(sender, memory_control, notifications)
    }
}
