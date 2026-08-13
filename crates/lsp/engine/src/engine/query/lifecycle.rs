//! Request lifecycle policy shared by every analysis query.
//!
//! Query methods only compute a result. This layer decides whether that work should run and
//! whether its result has an honest semantic meaning. It skips cancelled requests, releases
//! request-scoped package loads, reports stale saved-source input explicitly, and turns a
//! recoverable package-cache failure into a temporary operational abort followed by project repair.
//! Editor supersession is deliberately absent: only server ingress owns current open sessions and
//! revisions, so the engine tags a successful value and lets that owner validate publication.
//!
//! A saved-source race follows a different path. If hover discovers that `src/lib.rs` changed on
//! disk, hover returns `AnalysisAbort::SourceChanged`, records that the saved generation is stale,
//! and enqueues `src/lib.rs` on the ordinary path-change stream. Later queries return the same
//! operational abort until that mutation publishes a coherent generation; the hover itself never
//! turns into a synchronous reindex.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use rg_lsp_proto::{
    AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady, AnalysisScope,
    DocumentAnalysisSnapshot, EditorSnapshotRevision, TargetDocumentRevision,
};
use rg_project::Project;

use super::QueryRunner;
use crate::{engine::command::AnalysisResponse, memory::MemoryReporter};

/// Request identity captured before one command begins analysis.
///
/// Queue time is recorded separately from execution time. The lightweight editor identity here is
/// copied from the immutable command solely so it can be returned with a successful value.
#[derive(Debug)]
pub(crate) struct QueryContext {
    label: &'static str,
    queue_elapsed: Duration,
    scope: AnalysisScope,
    target: Option<(TargetDocumentRevision, EditorSnapshotRevision)>,
}

impl QueryContext {
    pub(crate) fn new(label: &'static str, queue_elapsed: Duration) -> Self {
        Self {
            label,
            queue_elapsed,
            scope: AnalysisScope::Workspace,
            target: None,
        }
    }

    pub(crate) fn document(
        label: &'static str,
        queue_elapsed: Duration,
        snapshot: &DocumentAnalysisSnapshot,
        scope: AnalysisScope,
    ) -> Self {
        Self {
            label,
            queue_elapsed,
            scope,
            target: Some((snapshot.target().clone(), snapshot.editor().revision())),
        }
    }

    fn analysis_input(&self, saved_project_generation: u64) -> AnalysisInput {
        match &self.target {
            Some((target, editor_revision)) => AnalysisInput::for_target_revision(
                saved_project_generation,
                *editor_revision,
                target.clone(),
                self.scope,
            ),
            None => AnalysisInput::for_saved_project(saved_project_generation),
        }
    }
}

impl QueryRunner<'_> {
    /// Run one read-only request through cancellation, recovery, and publication policy.
    ///
    /// The supplied closure contains only the feature query. A stale saved-source error latches the
    /// saved generation as stale and schedules normal path recovery instead of rebuilding inside
    /// this request. Once a result is safe to publish, its response channel is completed before
    /// request cleanup and recovery begin. Those steps still finish before the next engine command
    /// is accepted, but they no longer add to the completed request's response latency.
    pub(crate) fn respond_to_query<T>(
        &mut self,
        context: QueryContext,
        respond_to: AnalysisResponse<T>,
        query: impl FnOnce(&mut Self) -> anyhow::Result<T>,
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

        // From here on, saved and source-override requests share the same execution path. Keeping
        // the stale check in the context layer lets timing and cache recovery remain uniform for
        // every analysis query.
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
        self.analysis_scope = context.scope;
        let analysis_input = context.analysis_input(self.project.generation());
        let result = query(self);
        let stale_path = result
            .as_ref()
            .err()
            .and_then(Project::stale_source_path)
            .map(std::path::Path::to_path_buf);
        let stale_source_aborted = stale_path.is_some();
        if let Some(stale_path) = &stale_path {
            // A source race is a project-lifecycle event, not a feature-query failure. Re-enter the
            // FIFO mutation stream and abort this response while that recovery catches up.
            self.project.record_stale_source(label, stale_path);
        }
        let query_elapsed = started.elapsed();
        let should_recover = result
            .as_ref()
            .err()
            .is_some_and(Project::is_recoverable_cache_load_failure);
        if stale_source_aborted {
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

        if stale_source_aborted {
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
            self.project.recover_after_query_cache_failure(label);
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

    use rg_lsp_proto::{
        AnalysisOutcome, AnalysisScope, DocumentAnalysisSnapshot, DocumentRevision,
        EditorDocumentSnapshot, EditorSnapshot, EditorSnapshotRevision, OpenDocumentSession,
        ServiceNotification,
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
            |_| {
                query_ran.set(true);
                Ok(vec![1])
            },
        );

        assert!(!query_ran.get(), "cancelled query should not run analysis");
    }

    #[test]
    fn document_query_returns_the_exact_target_identity_used_by_analysis() {
        let document = EditorDocumentSnapshot::new(
            PathBuf::from("/workspace/src/lib.rs"),
            OpenDocumentSession::new(3),
            DocumentRevision::new(8),
            Some(5),
            "fn editor() {}".to_string(),
        );
        let snapshot = DocumentAnalysisSnapshot::new(
            document.target().clone(),
            EditorSnapshot::new(EditorSnapshotRevision::new(13), vec![document]),
        );
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) = oneshot::channel();
        let context = QueryContext::document(
            "hover",
            Duration::ZERO,
            &snapshot,
            AnalysisScope::ChangedPackages,
        );

        runner.respond_to_query(context, respond_to, |_| Ok(vec![1_usize]));

        let result = futures::executor::block_on(response)
            .expect("document query should send a response")
            .expect("document query should succeed");
        let AnalysisOutcome::Ready(ready) = result else {
            panic!("document query should be ready");
        };
        assert_eq!(ready.value(), &vec![1]);
        assert_eq!(ready.input().target_document(), Some(snapshot.target()));
        assert_eq!(
            ready.input().editor_snapshot_revision(),
            Some(snapshot.editor().revision())
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
            |_| Ok(Vec::<usize>::new()),
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

    fn test_project(memory_control: Arc<dyn MemoryControl>) -> ProjectCoordinator {
        let (sender, _receiver) = mpsc::channel();
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        ProjectCoordinator::new(sender, memory_control, notifications)
    }
}
