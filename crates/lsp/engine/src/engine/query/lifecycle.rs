//! Request lifecycle policy shared by every analysis query.
//!
//! Query methods only compute a result. This layer decides whether that work should run and
//! whether its result is still safe to publish. It skips cancelled or already-obsolete requests,
//! releases request-scoped package loads, neutralizes stale saved-source failures, and turns a
//! recoverable package-cache failure into an empty answer followed by project repair.
//!
//! Dirty identity is checked twice: once before analysis to avoid known-obsolete work, and once
//! after analysis so an editor change that arrived during a synchronous query cannot receive an
//! older hover, completion, or edit result.
//!
//! Example: hover for dirty version 7 starts, then `didChange` publishes version 8 while inference
//! is running. The computation is allowed to finish, but its version-7 result is replaced with the
//! feature's empty response instead of being sent to the editor.
//!
//! A saved-source race follows a different path. If hover discovers that `src/lib.rs` changed on
//! disk, hover returns its empty response, records that the saved generation is stale, and enqueues
//! `src/lib.rs` on the ordinary path-change stream. Later queries stay empty until that mutation
//! publishes a coherent generation; the hover itself never turns into a synchronous reindex.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use rg_project::Project;

use super::QueryRunner;
use crate::{
    dirty_state::{DirtyDocumentIdentity, DirtyState},
    documents::DirtyDocumentSnapshot,
    engine::command::EngineResponse,
    memory::MemoryReporter,
};

/// Request identity captured before one command begins analysis.
///
/// Queue time is recorded separately from execution time. Document queries also carry the exact
/// dirty snapshot they were created for, which lets publication reject a result even when a newer
/// `didChange` was processed while the query was running.
#[derive(Debug)]
pub(crate) struct QueryContext {
    label: &'static str,
    queue_elapsed: Duration,
    dirty_identity: Option<DirtyDocumentIdentity>,
}

impl QueryContext {
    pub(crate) fn new(label: &'static str, queue_elapsed: Duration) -> Self {
        Self {
            label,
            queue_elapsed,
            dirty_identity: None,
        }
    }

    pub(crate) fn document(
        label: &'static str,
        queue_elapsed: Duration,
        dirty: Option<&DirtyDocumentSnapshot>,
    ) -> Self {
        Self {
            label,
            queue_elapsed,
            dirty_identity: dirty.map(DirtyDocumentIdentity::from_snapshot),
        }
    }

    fn stale_dirty_identity(&self, dirty_state: &DirtyState) -> Option<&DirtyDocumentIdentity> {
        self.dirty_identity
            .as_ref()
            .filter(|identity| !dirty_state.is_current_identity(identity))
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
        respond_to: EngineResponse<T>,
        query: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) where
        T: Default + Send + 'static,
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

        // If a newer document version is already available, this queued dirty query can only
        // produce obsolete results.
        if let Some(dirty_identity) = context.stale_dirty_identity(self.dirty_state) {
            tracing::debug!(
                label = context.label,
                path = %dirty_identity.path().display(),
                version = ?dirty_identity.version(),
                text_len = dirty_identity.text_len(),
                queued_ms = context.queue_elapsed.as_millis(),
                "stale dirty analysis query skipped"
            );
            let _ = respond_to.send(Ok(T::default()));
            return;
        }

        // Once one query proves that the saved generation no longer describes disk, all later
        // queries are known to be unsafe. They remain cheap neutral responses until the queued
        // watcher/recovery mutation publishes a coherent generation.
        if let Some(stale_source) = self.project.stale_source() {
            tracing::debug!(
                label = context.label,
                path = %stale_source.display(),
                queued_ms = context.queue_elapsed.as_millis(),
                "analysis query skipped for stale saved generation"
            );
            let _ = respond_to.send(Ok(T::default()));
            return;
        }

        // From here on, clean and current-dirty requests share the same execution path. Keeping the
        // stale check in the context layer lets timing and cache recovery remain uniform for every
        // analysis query.
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
        let mut result = query(self);
        let stale_path = result
            .as_ref()
            .err()
            .and_then(Project::stale_source_path)
            .map(std::path::Path::to_path_buf);
        let stale_source_neutralized = stale_path.is_some();
        if let Some(stale_path) = stale_path {
            // A source race is a project-lifecycle event, not a feature-query failure. Re-enter the
            // FIFO mutation stream and keep this response neutral while that recovery catches up.
            self.project.record_stale_source(label, &stale_path);
            result = Ok(T::default());
        }
        let query_elapsed = started.elapsed();
        let should_recover = result
            .as_ref()
            .err()
            .is_some_and(Project::is_recoverable_cache_load_failure);
        match &result {
            Ok(_) => {
                tracing::info!(
                    query = label,
                    queued_ms = queue_elapsed.as_millis(),
                    elapsed_ms = query_elapsed.as_millis(),
                    status = "ok",
                    stale_source_neutralized,
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
                    stale_source_neutralized,
                    error = %error,
                    "analysis query completed"
                );
            }
        }

        // The document can change while synchronous analysis is running. Check the identity again
        // immediately before publication so an older hover, completion, or edit result does not
        // race a newer editor buffer back to the client.
        if let Some(dirty_identity) = context.stale_dirty_identity(self.dirty_state) {
            tracing::debug!(
                label,
                path = %dirty_identity.path().display(),
                version = ?dirty_identity.version(),
                text_len = dirty_identity.text_len(),
                "analysis query result discarded after document changed"
            );
            let _ = respond_to.send(Ok(T::default()));
        } else if should_recover {
            // Lazy package loads can fail when an offloaded artifact becomes stale between
            // indexing and a query. The next command sees a repaired project, while this request
            // degrades to an empty answer instead of a visible JSON-RPC popup in the editor.
            let _ = respond_to.send(Ok(T::default()));
        } else {
            let _ = respond_to.send(result);
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

    use rg_lsp_proto::ServiceNotification;
    use tokio::sync::oneshot;

    use super::QueryContext;
    use crate::{
        dirty_state::DirtyState,
        documents::{DirtyDocumentSnapshotState, DocumentStore},
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
    fn stale_dirty_query_responds_without_running_analysis() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut documents = DocumentStore::default();
        documents.did_open_saved(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(
            path.clone(),
            Some(2),
            Some("fn main() {\n    dirty();\n}\n"),
        );

        let DirtyDocumentSnapshotState::Dirty(snapshot) = documents.dirty_snapshot(&path) else {
            panic!("dirty full-sync document should expose a snapshot");
        };

        let dirty_state = DirtyState::default();
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, &dirty_state, memory_control);
        let (respond_to, response) = oneshot::channel();
        let context = QueryContext::document("hover", Duration::ZERO, Some(&snapshot));

        runner.respond_to_query(context, respond_to, |_| {
            panic!("stale query should not run analysis")
        });

        let result: Option<ls_types::Hover> = futures::executor::block_on(response)
            .expect("stale query should send a response")
            .expect("stale query should send a successful neutral result");
        assert!(result.is_none());
    }

    #[test]
    fn cancelled_query_does_not_run_analysis() {
        let dirty_state = DirtyState::default();
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, &dirty_state, memory_control);
        let (respond_to, response) = oneshot::channel::<anyhow::Result<Vec<usize>>>();
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
    fn dirty_query_result_is_discarded_if_document_changes_during_analysis() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut documents = DocumentStore::default();
        documents.did_open_saved(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(
            path.clone(),
            Some(2),
            Some("fn main() {\n    first();\n}\n"),
        );
        let first_dirty = documents.dirty_snapshot(&path);
        let DirtyDocumentSnapshotState::Dirty(first_snapshot) = &first_dirty else {
            panic!("dirty full-sync document should expose a snapshot");
        };

        let dirty_state = DirtyState::default();
        dirty_state.sync_document(&path, &first_dirty);
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let mut project = test_project(Arc::clone(&memory_control));
        let mut runner = QueryRunner::new(&mut project, &dirty_state, memory_control);
        let (respond_to, response) = oneshot::channel();
        let context = QueryContext::document("hover", Duration::ZERO, Some(first_snapshot));

        runner.respond_to_query(context, respond_to, |_| {
            documents.did_change(
                path.clone(),
                Some(3),
                Some("fn main() {\n    second();\n}\n"),
            );
            dirty_state.sync_document(&path, &documents.dirty_snapshot(&path));
            Ok(vec![1_usize])
        });

        let result = futures::executor::block_on(response)
            .expect("superseded query should send a response")
            .expect("superseded query should send a successful neutral result");
        assert!(result.is_empty());
    }

    fn test_project(memory_control: Arc<dyn MemoryControl>) -> ProjectCoordinator {
        let (sender, _receiver) = mpsc::channel();
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        ProjectCoordinator::new(sender, memory_control, notifications)
    }
}
