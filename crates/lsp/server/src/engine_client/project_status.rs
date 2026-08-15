//! Presentation status while a live engine process replaces its saved project.
//!
//! A native watcher can keep the process alive while saved-project work is pending. This module
//! counts overlapping foreground updates and publishes that state to the workspace status model.
//! Query correctness belongs to the engine's serialized command lane and typed outcomes, not this
//! presentation state.

use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use tokio::sync::watch;

/// Saved-project status displayed for one live engine process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EngineProjectStatus {
    /// No foreground saved-project update is active, and the last update succeeded.
    Ready,
    /// One or more foreground saved-project updates are pending or running.
    Updating,
    /// The last completed foreground update failed and no successful retry has replaced it.
    Failed(Arc<str>),
}

/// Shared presentation state behind all clones of one `EngineClient`.
///
/// The mutex keeps overlapping update counts and their last failure together. The watch channel
/// publishes the resulting active-workspace status transitions.
#[derive(Debug)]
pub(super) struct EngineProjectStatusState {
    inner: Mutex<EngineProjectStatusInner>,
    changes: watch::Sender<EngineProjectStatus>,
}

#[derive(Debug, Default)]
struct EngineProjectStatusInner {
    active_updates: usize,
    failure: Option<Arc<str>>,
}

impl EngineProjectStatusState {
    pub(super) fn new() -> Self {
        let (changes, _) = watch::channel(EngineProjectStatus::Ready);
        Self {
            inner: Mutex::new(EngineProjectStatusInner::default()),
            changes,
        }
    }

    pub(super) fn current(&self) -> EngineProjectStatus {
        self.changes.borrow().clone()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<EngineProjectStatus> {
        self.changes.subscribe()
    }

    pub(super) fn begin(self: &Arc<Self>) -> EngineProjectUpdate {
        let mut inner = self
            .inner
            .lock()
            .expect("engine project status mutex should not be poisoned");
        inner.active_updates += 1;
        tracing::debug!(
            active_updates = inner.active_updates,
            "engine project update started"
        );
        self.publish(EngineProjectStatus::Updating);
        drop(inner);

        EngineProjectUpdate {
            status: Arc::clone(self),
            finished: false,
        }
    }

    fn finish(&self, outcome: EngineProjectUpdateOutcome) {
        let mut inner = self
            .inner
            .lock()
            .expect("engine project status mutex should not be poisoned");
        assert!(
            inner.active_updates > 0,
            "finished project update should have been started"
        );
        inner.active_updates -= 1;
        let outcome_label = match &outcome {
            EngineProjectUpdateOutcome::Succeeded => "succeeded",
            EngineProjectUpdateOutcome::Failed(_) => "failed",
            EngineProjectUpdateOutcome::Cancelled => "cancelled",
        };
        match outcome {
            EngineProjectUpdateOutcome::Succeeded => inner.failure = None,
            EngineProjectUpdateOutcome::Failed(failure) => inner.failure = Some(failure),
            EngineProjectUpdateOutcome::Cancelled => {}
        }

        let status = if inner.active_updates > 0 {
            EngineProjectStatus::Updating
        } else if let Some(failure) = &inner.failure {
            EngineProjectStatus::Failed(Arc::clone(failure))
        } else {
            EngineProjectStatus::Ready
        };
        tracing::debug!(
            active_updates = inner.active_updates,
            outcome = outcome_label,
            status = ?status,
            "engine project update finished"
        );
        self.publish(status);
    }

    fn publish(&self, next: EngineProjectStatus) {
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
#[must_use = "dropping a project update immediately cancels it"]
#[derive(Debug)]
pub(crate) struct EngineProjectUpdate {
    status: Arc<EngineProjectStatusState>,
    finished: bool,
}

impl EngineProjectUpdate {
    /// Keep this update live until accepted engine work actually returns.
    ///
    /// The small task is intentional. Dropping the server request that awaits this method detaches
    /// its join handle, but the task retains both the engine RPC future and this update. The
    /// workspace status therefore cannot return to ready while a mutation already accepted by the
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
        .context("join project update")?
    }

    pub(crate) fn finish(mut self) {
        self.complete(EngineProjectUpdateOutcome::Succeeded);
    }

    pub(crate) fn fail(mut self, error: &anyhow::Error) {
        self.complete(EngineProjectUpdateOutcome::Failed(Arc::from(format!(
            "{error:#}"
        ))));
    }

    fn complete(&mut self, outcome: EngineProjectUpdateOutcome) {
        self.status.finish(outcome);
        self.finished = true;
    }
}

impl Drop for EngineProjectUpdate {
    fn drop(&mut self) {
        if !self.finished {
            self.status.finish(EngineProjectUpdateOutcome::Cancelled);
        }
    }
}

#[derive(Debug)]
enum EngineProjectUpdateOutcome {
    Succeeded,
    Failed(Arc<str>),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::oneshot;

    use super::{EngineProjectStatus, EngineProjectStatusState};

    #[test]
    fn overlapping_updates_publish_only_the_outer_status_transitions() {
        let status = Arc::new(EngineProjectStatusState::new());

        let first = status.begin();
        let second = status.begin();
        assert_eq!(status.current(), EngineProjectStatus::Updating);

        first.finish();
        assert_eq!(
            status.current(),
            EngineProjectStatus::Updating,
            "one remaining update should keep the project status active"
        );
        second.finish();
        assert_eq!(status.current(), EngineProjectStatus::Ready);
    }

    #[test]
    fn failed_update_stays_failed_until_a_successful_retry() {
        let status = Arc::new(EngineProjectStatusState::new());
        status
            .begin()
            .fail(&anyhow::anyhow!("source generation kept changing"));
        assert!(matches!(status.current(), EngineProjectStatus::Failed(_)));

        status.begin().finish();
        assert_eq!(status.current(), EngineProjectStatus::Ready);
    }

    #[test]
    fn unrelated_cancelled_update_does_not_clear_a_failure() {
        let status = Arc::new(EngineProjectStatusState::new());
        status
            .begin()
            .fail(&anyhow::anyhow!("project update failed"));

        drop(status.begin());

        assert!(matches!(status.current(), EngineProjectStatus::Failed(_)));
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_finish_accepted_project_update() {
        let status = Arc::new(EngineProjectStatusState::new());
        let update = status.begin();
        let mut changes = status.subscribe();
        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = oneshot::channel();

        let waiter = tokio::spawn(update.run_to_completion(async move {
            started_sender
                .send(())
                .expect("project update test should still await its start signal");
            release_receiver
                .await
                .expect("project update test should release accepted work");
            Ok::<(), anyhow::Error>(())
        }));
        started_receiver
            .await
            .expect("project update should start before cancellation");

        waiter.abort();
        assert!(
            waiter
                .await
                .expect_err("cancelled waiter should stop")
                .is_cancelled()
        );
        assert_eq!(status.current(), EngineProjectStatus::Updating);

        release_sender
            .send(())
            .expect("detached project update should still be waiting");
        changes
            .changed()
            .await
            .expect("finishing detached work should publish project status");
        assert_eq!(status.current(), EngineProjectStatus::Ready);
    }
}
