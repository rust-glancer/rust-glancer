//! Query admission while a live engine process replaces its saved project.
//!
//! Transport readiness is not enough to answer an LSP request. A native watcher can keep the
//! process alive while the only coherent saved generation is being replaced. This module publishes
//! that narrower availability, counts overlapping foreground updates, and lets pending interactive
//! RPCs stop waiting without adding another semantic execution lane.

use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use tokio::sync::watch;

/// Query-facing state of a process whose transport may remain fully alive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EngineAvailability {
    /// The saved project is coherent and interactive queries may run.
    Queryable,
    /// One or more foreground saved-project updates are pending or running.
    Indexing,
    /// The last completed foreground update failed and no successful retry has replaced it.
    Unavailable(Arc<str>),
}

impl EngineAvailability {
    pub(super) fn is_queryable(&self) -> bool {
        matches!(self, Self::Queryable)
    }
}

/// Availability plus a monotonic foreground-update identity.
///
/// The revision changes when indexing begins, not when it ends. An interactive request can
/// therefore reject a response computed across a complete indexing transition even if the latest
/// state has already returned to `Queryable`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EngineAvailabilitySnapshot {
    pub(super) availability: EngineAvailability,
    revision: u64,
}

/// Shared availability state behind all clones of one `EngineClient`.
///
/// The mutex keeps overlapping update counts and their last failure together. The watch channel
/// publishes only the query-facing snapshot, so both waiting queries and the active-workspace
/// status monitor observe the same transitions.
#[derive(Debug)]
pub(super) struct EngineAvailabilityState {
    inner: Mutex<EngineAvailabilityInner>,
    changes: watch::Sender<EngineAvailabilitySnapshot>,
}

#[derive(Debug, Default)]
struct EngineAvailabilityInner {
    active_indexing: usize,
    revision: u64,
    failure: Option<Arc<str>>,
}

impl EngineAvailabilityState {
    pub(super) fn new() -> Self {
        let initial = EngineAvailabilitySnapshot {
            availability: EngineAvailability::Queryable,
            revision: 0,
        };
        let (changes, _) = watch::channel(initial);
        Self {
            inner: Mutex::new(EngineAvailabilityInner::default()),
            changes,
        }
    }

    pub(super) fn current(&self) -> EngineAvailabilitySnapshot {
        self.changes.borrow().clone()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<EngineAvailabilitySnapshot> {
        self.changes.subscribe()
    }

    /// Run an interactive RPC only while it belongs to one queryable availability revision.
    ///
    /// Returning `None` asks `EngineClient::query` to use the feature's neutral result. The
    /// subscription is created before the RPC is polled, then both completion and every published
    /// transition are checked against the starting revision. This catches even a complete
    /// `Queryable -> Indexing -> Queryable` transition between two polls.
    pub(super) async fn run_query<T>(
        &self,
        operation: &'static str,
        request: impl Future<Output = T>,
    ) -> Option<T> {
        let mut availability = self.subscribe();
        let initial = availability.borrow().clone();
        if !initial.availability.is_queryable() {
            tracing::debug!(
                operation,
                availability = ?initial.availability,
                "analysis query skipped while project is unavailable"
            );
            return None;
        }

        tokio::pin!(request);
        loop {
            tokio::select! {
                result = &mut request => {
                    let current = availability.borrow().clone();
                    if current.revision != initial.revision
                        || !current.availability.is_queryable()
                    {
                        return None;
                    }
                    return Some(result);
                },
                changed = availability.changed() => {
                    if changed.is_err() {
                        return Some(request.await);
                    }

                    let current = availability.borrow().clone();
                    // Even a very short indexing transition invalidates an older response. The
                    // revision catches `queryable -> indexing -> queryable` when both transitions
                    // happen before this task gets polled again.
                    if current.revision != initial.revision
                        || !current.availability.is_queryable()
                    {
                        tracing::debug!(
                            operation,
                            availability = ?current.availability,
                            "analysis query neutralized after project availability changed"
                        );
                        return None;
                    }
                }
            }
        }
    }

    pub(super) fn begin(self: &Arc<Self>) -> EngineIndexingActivity {
        let mut inner = self
            .inner
            .lock()
            .expect("engine availability mutex should not be poisoned");
        if inner.active_indexing == 0 {
            inner.revision = inner.revision.wrapping_add(1);
        }
        inner.active_indexing += 1;
        self.publish(EngineAvailabilitySnapshot {
            availability: EngineAvailability::Indexing,
            revision: inner.revision,
        });
        drop(inner);

        EngineIndexingActivity {
            availability: Arc::clone(self),
            finished: false,
        }
    }

    fn finish(&self, outcome: EngineIndexingOutcome) {
        let mut inner = self
            .inner
            .lock()
            .expect("engine availability mutex should not be poisoned");
        assert!(
            inner.active_indexing > 0,
            "finished indexing activity should have been started"
        );
        inner.active_indexing -= 1;
        match outcome {
            EngineIndexingOutcome::Succeeded => inner.failure = None,
            EngineIndexingOutcome::Failed(failure) => inner.failure = Some(failure),
            EngineIndexingOutcome::Cancelled => {}
        }

        let availability = if inner.active_indexing > 0 {
            EngineAvailability::Indexing
        } else if let Some(failure) = &inner.failure {
            EngineAvailability::Unavailable(Arc::clone(failure))
        } else {
            EngineAvailability::Queryable
        };
        self.publish(EngineAvailabilitySnapshot {
            availability,
            revision: inner.revision,
        });
    }

    fn publish(&self, next: EngineAvailabilitySnapshot) {
        self.changes.send_if_modified(|current| {
            if *current == next {
                return false;
            }
            *current = next;
            true
        });
    }
}

/// One counted foreground saved-project update.
///
/// `finish` clears an earlier failure after a successful retry. `fail` retains the formatted error
/// once the last overlapping update ends. Dropping an unfinished guard only cancels its own count;
/// it deliberately does not turn cancellation into success or erase a failure from another update.
#[must_use = "dropping an indexing activity immediately cancels it"]
#[derive(Debug)]
pub(crate) struct EngineIndexingActivity {
    availability: Arc<EngineAvailabilityState>,
    finished: bool,
}

impl EngineIndexingActivity {
    /// Keep this activity live until accepted engine work actually returns.
    ///
    /// The small task is intentional. Dropping the server request that awaits this method detaches
    /// its join handle, but the task retains both the engine RPC future and this activity. The
    /// workspace therefore cannot become queryable while a mutation already accepted by the
    /// engine is still queued or running.
    pub(crate) async fn run_to_completion<T>(
        self,
        work: impl Future<Output = anyhow::Result<T>> + Send + 'static,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        tokio::spawn(async move {
            let result = work.await;
            match &result {
                Ok(_) => self.finish(),
                Err(error) => self.fail(error),
            }
            result
        })
        .await
        .context("join indexing activity")?
    }

    pub(crate) fn finish(mut self) {
        self.complete(EngineIndexingOutcome::Succeeded);
    }

    pub(crate) fn fail(mut self, error: &anyhow::Error) {
        self.complete(EngineIndexingOutcome::Failed(Arc::from(format!(
            "{error:#}"
        ))));
    }

    fn complete(&mut self, outcome: EngineIndexingOutcome) {
        self.availability.finish(outcome);
        self.finished = true;
    }
}

impl Drop for EngineIndexingActivity {
    fn drop(&mut self) {
        if !self.finished {
            self.availability.finish(EngineIndexingOutcome::Cancelled);
        }
    }
}

#[derive(Debug)]
enum EngineIndexingOutcome {
    Succeeded,
    Failed(Arc<str>),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::oneshot;

    use super::{EngineAvailability, EngineAvailabilitySnapshot, EngineAvailabilityState};

    #[test]
    fn indexing_activities_publish_only_the_outer_transitions() {
        let availability = Arc::new(EngineAvailabilityState::new());

        let first = availability.begin();
        let revision = availability.current().revision;
        let second = availability.begin();
        assert_eq!(
            availability.current().availability,
            EngineAvailability::Indexing
        );

        first.finish();
        assert_eq!(
            availability.current().availability,
            EngineAvailability::Indexing,
            "one remaining activity should keep queries unavailable"
        );
        second.finish();
        assert_eq!(
            availability.current(),
            EngineAvailabilitySnapshot {
                availability: EngineAvailability::Queryable,
                revision,
            }
        );
    }

    #[test]
    fn failed_indexing_stays_unavailable_until_a_successful_retry() {
        let availability = Arc::new(EngineAvailabilityState::new());
        availability
            .begin()
            .fail(&anyhow::anyhow!("source generation kept changing"));
        assert!(matches!(
            availability.current().availability,
            EngineAvailability::Unavailable(_)
        ));

        availability.begin().finish();
        assert_eq!(
            availability.current().availability,
            EngineAvailability::Queryable
        );
    }

    #[test]
    fn unrelated_cancelled_activity_does_not_clear_an_indexing_failure() {
        let availability = Arc::new(EngineAvailabilityState::new());
        availability
            .begin()
            .fail(&anyhow::anyhow!("project update failed"));

        drop(availability.begin());

        assert!(matches!(
            availability.current().availability,
            EngineAvailability::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn indexing_neutralizes_an_already_waiting_query() {
        let availability = Arc::new(EngineAvailabilityState::new());
        let query = tokio::spawn({
            let availability = Arc::clone(&availability);
            async move {
                availability
                    .run_query("hover", std::future::pending::<()>())
                    .await
            }
        });
        tokio::task::yield_now().await;

        let activity = availability.begin();
        assert_eq!(
            query.await.expect("query task should finish"),
            None,
            "the waiting query should not survive an indexing transition"
        );
        drop(activity);
    }

    #[tokio::test]
    async fn completed_indexing_transition_still_invalidates_an_older_query() {
        let availability = Arc::new(EngineAvailabilityState::new());
        let query = tokio::spawn({
            let availability = Arc::clone(&availability);
            async move {
                availability
                    .run_query("document_symbol", std::future::pending::<()>())
                    .await
            }
        });
        tokio::task::yield_now().await;

        availability.begin().finish();
        assert_eq!(
            query.await.expect("query task should finish"),
            None,
            "the availability revision should preserve a short indexing transition"
        );
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_finish_accepted_indexing_work() {
        let availability = Arc::new(EngineAvailabilityState::new());
        let activity = availability.begin();
        let mut changes = availability.subscribe();
        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = oneshot::channel();

        let waiter = tokio::spawn(activity.run_to_completion(async move {
            started_sender
                .send(())
                .expect("indexing test should still await its start signal");
            release_receiver
                .await
                .expect("indexing test should release accepted work");
            Ok::<(), anyhow::Error>(())
        }));
        started_receiver
            .await
            .expect("indexing work should start before cancellation");

        waiter.abort();
        assert!(
            waiter
                .await
                .expect_err("cancelled waiter should stop")
                .is_cancelled()
        );
        assert_eq!(
            availability.current().availability,
            EngineAvailability::Indexing,
            "accepted work should retain its activity after the waiter is cancelled"
        );

        release_sender
            .send(())
            .expect("detached indexing work should still be waiting");
        changes
            .changed()
            .await
            .expect("finishing detached work should publish availability");
        assert_eq!(
            availability.current().availability,
            EngineAvailability::Queryable
        );
    }
}
