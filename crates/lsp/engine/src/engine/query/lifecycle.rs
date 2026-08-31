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
//! disk, hover returns `QueryError::SavedSourceChanged`, records that the saved generation is stale,
//! and enqueues `src/lib.rs` on the normal path-change stream. Later queries return the same error
//! until that mutation publishes a new saved project; the hover itself never turns into a
//! synchronous reindex.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use rg_lsp_proto::{
    EditorDocumentSnapshot, EngineError, GlobalPositionSnapshot, QueryError, QueryScope, QueryValue,
};
use rg_project::Project;
use rg_std::CancellationToken;

use super::QueryRunner;
use crate::{engine::command::QueryResponder, memory::MemoryReporter};

/// Error kept by `QueryRunner` until the query lifecycle classifies it for the protocol.
///
/// Most failures keep their rich `anyhow` chain while this layer checks for cancellation, stale
/// source, and recoverable cache failures. Operations that require saved editor text use the
/// separate variant because that is an expected request outcome, not an engine failure.
#[derive(Debug)]
pub(crate) enum QueryRunError {
    SaveRequired(std::path::PathBuf),
    Analysis(anyhow::Error),
}

impl QueryRunError {
    fn as_error(&self) -> Option<&anyhow::Error> {
        match self {
            Self::SaveRequired(_) => None,
            Self::Analysis(error) => Some(error),
        }
    }
}

impl From<anyhow::Error> for QueryRunError {
    fn from(error: anyhow::Error) -> Self {
        Self::Analysis(error)
    }
}

/// Lets expensive query phases check whether anyone still wants the result.
///
/// Dropping the RPC future cancels the request token and closes the engine response channel.
/// Ordinary query phases check both here. The token can also enter bounded semantic loops where
/// polling the response endpoint would cross an engine-layer ownership boundary.
pub(crate) struct QueryCancellation<'a> {
    request: &'a CancellationToken,
    response_is_closed: &'a dyn Fn() -> bool,
}

impl<'a> QueryCancellation<'a> {
    fn new(request: &'a CancellationToken, response_is_closed: &'a dyn Fn() -> bool) -> Self {
        Self {
            request,
            response_is_closed,
        }
    }

    /// Stop at a named query boundary if nobody can receive the result anymore.
    pub(crate) fn checkpoint(&self, checkpoint: &'static str) -> anyhow::Result<()> {
        if self.request.is_cancelled() || (self.response_is_closed)() {
            return Err(QueryCancelled { checkpoint }.into());
        }
        Ok(())
    }

    /// Share the request signal with synchronous work that has its own bounded checkpoints.
    pub(crate) fn token(&self) -> CancellationToken {
        self.request.clone()
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
/// Queue time is kept separate from execution time. The scope is copied from the command so a
/// successful result can tell the server which editor state needs a final check.
#[derive(Debug)]
pub(crate) struct QueryContext {
    label: &'static str,
    queue_elapsed: Duration,
    scope: QueryScope,
}

impl QueryContext {
    pub(crate) fn saved_project(label: &'static str, queue_elapsed: Duration) -> Self {
        Self {
            label,
            queue_elapsed,
            scope: QueryScope::SavedProject,
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
            scope: QueryScope::GlobalOperation {
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
            scope: QueryScope::TargetDocument(document.target().clone()),
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
        respond_to: QueryResponder<T>,
        cancellation: CancellationToken,
        query: impl FnOnce(&mut Self, &QueryCancellation<'_>) -> Result<T, QueryRunError>,
    ) where
        T: Send + 'static,
    {
        let QueryContext {
            label,
            queue_elapsed,
            scope,
        } = context;

        // LSP cancellation drops the RPC handler waiting on this response. The command may still
        // be in the dispatcher queue, but there is no reason to materialize packages or run
        // analysis once nobody can receive the result.
        if respond_to.is_closed() || cancellation.is_cancelled() {
            tracing::debug!(
                label,
                queued_ms = queue_elapsed.as_millis(),
                "cancelled analysis query skipped"
            );
            return;
        }

        // Once one query proves that the saved generation no longer describes disk, all later
        // queries are known to be unsafe. They return the same cheap error until the queued
        // watcher/recovery mutation publishes a coherent generation.
        if let Some(stale_source) = self.project.stale_source() {
            tracing::debug!(
                label,
                path = %stale_source.display(),
                queued_ms = queue_elapsed.as_millis(),
                "analysis query skipped for stale saved generation"
            );
            let _ = respond_to.send(Err(QueryError::SavedSourceChanged));
            return;
        }

        // Keeping the stale check in the context layer lets timing and cache recovery remain
        // uniform for every analysis query.
        tracing::trace!(
            label,
            queued_ms = queue_elapsed.as_millis(),
            "analysis query started"
        );
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let memory_before = MemoryReporter::snapshot(memory_control.as_ref());
        let result = {
            let response_is_closed = || respond_to.is_closed();
            query(
                self,
                &QueryCancellation::new(&cancellation, &response_is_closed),
            )
        };
        let cancelled_checkpoint = result
            .as_ref()
            .err()
            .and_then(QueryRunError::as_error)
            .and_then(|error| error.downcast_ref::<QueryCancelled>())
            .map(|cancelled| cancelled.checkpoint);
        let stale_path = if cancelled_checkpoint.is_some() {
            None
        } else {
            result
                .as_ref()
                .err()
                .and_then(QueryRunError::as_error)
                .and_then(Project::stale_source_path)
                .map(std::path::Path::to_path_buf)
        };
        let saved_source_changed = stale_path.is_some();
        if let Some(stale_path) = &stale_path {
            // A source race is a project-lifecycle event, not a feature-query failure. Re-enter the
            // FIFO mutation stream and stop this response while that recovery catches up.
            self.project.record_stale_source(label, stale_path);
        }
        let query_elapsed = started.elapsed();
        let should_recover = cancelled_checkpoint.is_none()
            && result
                .as_ref()
                .err()
                .and_then(QueryRunError::as_error)
                .is_some_and(Project::is_recoverable_cache_load_failure);
        if let Some(checkpoint) = cancelled_checkpoint {
            tracing::debug!(
                query = label,
                queued_ms = queue_elapsed.as_millis(),
                elapsed_ms = query_elapsed.as_millis(),
                checkpoint,
                "cancelled analysis query stopped"
            );
        } else if saved_source_changed {
            tracing::info!(
                query = label,
                queued_ms = queue_elapsed.as_millis(),
                elapsed_ms = query_elapsed.as_millis(),
                status = "saved_source_changed",
                error = ?QueryError::SavedSourceChanged,
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
                Err(QueryRunError::SaveRequired(path)) => {
                    tracing::debug!(
                        query = label,
                        queued_ms = queue_elapsed.as_millis(),
                        elapsed_ms = query_elapsed.as_millis(),
                        status = "save_required",
                        path = %path.display(),
                        "analysis query completed"
                    );
                }
                Err(QueryRunError::Analysis(error)) => {
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
            // query error or an empty feature result, so there is deliberately nothing to publish.
        } else if saved_source_changed {
            let _ = respond_to.send(Err(QueryError::SavedSourceChanged));
        } else if should_recover {
            // Lazy package loads can fail when an offloaded artifact becomes stale between
            // indexing and a query. The next command sees a repaired project; this request remains
            // explicitly unavailable instead of pretending the feature found no result.
            let _ = respond_to.send(Err(QueryError::TemporarilyUnavailable));
        } else {
            let result = match result {
                Ok(value) => Ok(QueryValue::new(value, scope)),
                Err(QueryRunError::SaveRequired(path)) => Err(QueryError::SaveRequired { path }),
                Err(QueryRunError::Analysis(error)) => {
                    Err(QueryError::Internal(EngineError::from(error)))
                }
            };
            let _ = respond_to.send(result);
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
        DocumentRevision, EditorDocumentSnapshot, GlobalPositionSnapshot, OpenDocumentSession,
        OpenDocumentsRevision, QueryError, QueryScope, QueryValue, ServiceNotification,
    };
    use rg_std::CancellationToken;
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
            oneshot::channel::<Result<QueryValue<Vec<usize>>, QueryError>>();
        drop(response);
        let query_ran = Cell::new(false);

        runner.respond_to_query(
            QueryContext::saved_project("workspace_symbol", Duration::ZERO),
            respond_to,
            CancellationToken::new(),
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

        runner.respond_to_query(context, respond_to, CancellationToken::new(), |_, _| {
            Ok(vec![1_usize])
        });

        let result =
            futures::executor::block_on(response).expect("document query should send a response");
        let response = result.expect("document query should succeed");
        assert_eq!(response.value(), &vec![1]);
        assert_eq!(
            response.scope(),
            &QueryScope::GlobalOperation {
                target: snapshot.target().clone(),
                open_documents_revision: snapshot.open_documents_revision(),
            }
        );
    }

    #[test]
    fn valid_empty_query_remains_successful() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) = oneshot::channel();

        runner.respond_to_query(
            QueryContext::saved_project("workspace_symbol", Duration::ZERO),
            respond_to,
            CancellationToken::new(),
            |_, _| Ok(Vec::<usize>::new()),
        );

        let result = futures::executor::block_on(response)
            .expect("valid empty query should send a response");
        let response = result.expect("valid empty query should remain successful");
        assert!(response.value().is_empty());
        assert_eq!(response.scope(), &QueryScope::SavedProject);
    }

    #[test]
    fn query_stops_when_response_closes_during_analysis() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, response) =
            oneshot::channel::<Result<QueryValue<Vec<usize>>, QueryError>>();
        let work_after_checkpoint_ran = Cell::new(false);

        runner.respond_to_query(
            QueryContext::saved_project("completion", Duration::ZERO),
            respond_to,
            CancellationToken::new(),
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

    #[test]
    fn query_stops_when_request_token_is_cancelled_during_analysis() {
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, memory_control);
        let (respond_to, _response) =
            oneshot::channel::<Result<QueryValue<Vec<usize>>, QueryError>>();
        let cancellation = CancellationToken::new();
        let request_owner = cancellation.clone();
        let work_after_checkpoint_ran = Cell::new(false);

        runner.respond_to_query(
            QueryContext::saved_project("inlay_hint", Duration::ZERO),
            respond_to,
            cancellation,
            |_, cancellation| {
                request_owner.cancel();
                cancellation
                    .checkpoint("test semantic work")
                    .context("stop test query after request cancellation")?;
                work_after_checkpoint_ran.set(true);
                Ok(vec![1])
            },
        );

        assert!(
            !work_after_checkpoint_ran.get(),
            "work after a cancelled request checkpoint should not run",
        );
    }

    fn test_project(memory_control: Arc<dyn MemoryControl>) -> ProjectCoordinator {
        let (sender, _receiver) = mpsc::channel();
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        ProjectCoordinator::new(sender, memory_control, notifications)
    }
}
