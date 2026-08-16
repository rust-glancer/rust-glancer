//! The bounded queue for each open document session.
//!
//! A session remembers the newest logical completion request captured at ingress. Its work is
//! either idle or consists of one active engine attempt plus, at most, the latest pending attempt.
//! This module owns those queue transitions. Each individual attempt owns the mechanics needed to
//! run its engine future and share its outcome.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

use rg_lsp_proto::OpenDocumentSession;

use super::{
    attempt::{
        ActiveAttempt, AttemptIdentity, AttemptJob, AttemptKey, AttemptStopReason,
        AttemptStopSignal, AttemptWaiter, RunningAttempt,
    },
    request::{CompletionAttemptOutcome, CompletionFuture, CompletionRequestLease},
};
use crate::ingress::DocumentRevisionWatch;

/// Identity assigned to one logical completion request at ingress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RequestId(pub(super) u64);

/// Requests compete only when they belong to the same open document session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SessionKey {
    pub(super) path: PathBuf,
    pub(super) session: OpenDocumentSession,
}

/// All completion scheduling state, partitioned by open document session.
#[derive(Default)]
pub(super) struct SchedulerState {
    next_request: u64,
    sessions: HashMap<SessionKey, SessionQueue>,
}

impl SchedulerState {
    /// Capture a logical request in wire order, then wake old work after releasing the lock.
    pub(super) fn capture_request(
        state: &Arc<Mutex<Self>>,
        key: AttemptKey,
    ) -> Arc<CompletionRequestLease> {
        let transition = {
            let mut state_guard = state
                .lock()
                .expect("completion scheduler mutex should not be poisoned");
            let session = key.session.clone();

            if let Some(request) = state_guard
                .sessions
                .get(&session)
                .and_then(|queue| queue.current.as_ref())
                .filter(|current| current.initial == key)
                .and_then(|current| current.lease.upgrade())
            {
                CaptureTransition::Duplicate { request, key }
            } else {
                let id = state_guard.next_request_id();
                let request = Arc::new(CompletionRequestLease::new(
                    id,
                    session.clone(),
                    Arc::downgrade(state),
                ));
                let queue = state_guard.sessions.entry(session).or_default();
                let previous = queue
                    .current
                    .replace(CurrentRequest {
                        id,
                        initial: key.clone(),
                        lease: Arc::downgrade(&request),
                    })
                    .and_then(|current| current.lease.upgrade());

                // Mark the old request while the queue transition is locked. An old handler
                // cannot publish before the wake-up signals below are delivered.
                if let Some(previous) = &previous {
                    previous.mark_replaced();
                }
                let (pending, active_stop) = queue.replace_request_work();
                CaptureTransition::New {
                    request,
                    key,
                    replaced_existing_request: previous.is_some(),
                    pending,
                    active_stop,
                }
            }
        };

        transition.apply()
    }

    /// Reuse matching work or place this attempt into the session's single pending position.
    pub(super) fn enqueue_attempt(
        request: &CompletionRequestLease,
        key: AttemptKey,
        invalidation: DocumentRevisionWatch,
        run: CompletionFuture,
    ) -> AttemptWaiter {
        if request.is_replaced() {
            return AttemptWaiter::ready(CompletionAttemptOutcome::Replaced);
        }

        // The editor may have changed before the handler reached the scheduler. Return the same
        // retry signal that an already-running attempt receives when its snapshot becomes old.
        if invalidation.is_superseded() {
            tracing::debug!(
                path = %key.session.path.display(),
                session = key.session.session.get(),
                revision = key.document_revision.get(),
                line = key.line,
                character = key.character,
                "completion attempt was obsolete at scheduler admission"
            );
            return AttemptWaiter::ready(CompletionAttemptOutcome::DocumentAdvanced);
        }

        let Some(state) = request.scheduler().upgrade() else {
            return AttemptWaiter::ready(CompletionAttemptOutcome::Replaced);
        };
        let (job, waiter) = AttemptJob::new(request.id, key, invalidation, run);
        let session = job.identity().key.session.clone();
        let logged_key = job.identity().key.clone();
        let action = {
            let mut state_guard = state
                .lock()
                .expect("completion scheduler mutex should not be poisoned");
            let Some(queue) = state_guard.sessions.get_mut(&session) else {
                return AttemptWaiter::ready(CompletionAttemptOutcome::Replaced);
            };
            if !queue
                .current
                .as_ref()
                .is_some_and(|current| current.id == request.id)
            {
                return AttemptWaiter::ready(CompletionAttemptOutcome::Replaced);
            }

            queue.schedule(job)
        };

        match action {
            ScheduleAction::ShareActive(shared) => {
                tracing::trace!(
                    path = %logged_key.session.path.display(),
                    session = logged_key.session.session.get(),
                    revision = logged_key.document_revision.get(),
                    line = logged_key.line,
                    character = logged_key.character,
                    "coalesced duplicate completion with active semantic attempt"
                );
                shared
            }
            ScheduleAction::SharePending(shared) => {
                tracing::trace!(
                    path = %logged_key.session.path.display(),
                    session = logged_key.session.session.get(),
                    revision = logged_key.document_revision.get(),
                    line = logged_key.line,
                    character = logged_key.character,
                    "coalesced duplicate completion with pending semantic attempt"
                );
                shared
            }
            ScheduleAction::Start(running) => {
                tokio::spawn(Self::run_session(state, session, running));
                waiter
            }
            ScheduleAction::Queue {
                replaced,
                stop,
                reason,
            } => {
                if let Some(replaced) = replaced {
                    let identity = replaced.identity();
                    tracing::trace!(
                        path = %identity.key.session.path.display(),
                        session = identity.key.session.session.get(),
                        revision = identity.key.document_revision.get(),
                        line = identity.key.line,
                        character = identity.key.character,
                        "replaced pending completion attempt"
                    );
                    let outcome = if identity.request == request.id {
                        CompletionAttemptOutcome::DocumentAdvanced
                    } else {
                        CompletionAttemptOutcome::Replaced
                    };
                    replaced.complete(outcome);
                }
                stop.send(reason);
                waiter
            }
        }
    }

    /// Run attempts serially until this session has no pending work.
    async fn run_session(
        state: Arc<Mutex<Self>>,
        session: SessionKey,
        mut running: RunningAttempt,
    ) {
        loop {
            let identity = running.identity().clone();
            let outcome_publisher = running.outcome_publisher().clone();
            let outcome = running.run().await;
            drop(running);

            // Clear the active state before waking handlers. A retry submitted by a woken handler
            // then observes the queue after this attempt, never a half-finished active entry.
            let next = state
                .lock()
                .expect("completion scheduler mutex should not be poisoned")
                .finish_attempt(&session, &identity);
            if let Some(outcome) = outcome {
                outcome_publisher.complete(outcome);
            }

            let Some(next) = next else {
                return;
            };
            running = next;
        }
    }

    /// Remove one finished active attempt and promote the latest pending attempt, if any.
    fn finish_attempt(
        &mut self,
        session: &SessionKey,
        identity: &AttemptIdentity,
    ) -> Option<RunningAttempt> {
        let queue = self
            .sessions
            .get_mut(session)
            .expect("active completion session should remain registered");
        let next = queue.finish_attempt(identity);
        if next.is_none() && queue.current.is_none() && matches!(queue.work, QueueWork::Idle) {
            self.sessions.remove(session);
        }
        next
    }

    /// Release this request only if it still owns the session's logical-request position.
    pub(super) fn release_request(&mut self, session: &SessionKey, id: RequestId) {
        let remove_session = self.sessions.get_mut(session).is_some_and(|queue| {
            if queue
                .current
                .as_ref()
                .is_some_and(|current| current.id == id)
            {
                queue.current = None;
            }
            queue.current.is_none() && matches!(queue.work, QueueWork::Idle)
        });
        if remove_session {
            self.sessions.remove(session);
        }
    }

    fn next_request_id(&mut self) -> RequestId {
        self.next_request = self
            .next_request
            .checked_add(1)
            .expect("completion request counter should not overflow");
        RequestId(self.next_request)
    }

    #[cfg(test)]
    pub(super) fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

/// Capture decision made under the mutex and completed after the mutex is released.
enum CaptureTransition {
    Duplicate {
        request: Arc<CompletionRequestLease>,
        key: AttemptKey,
    },
    New {
        request: Arc<CompletionRequestLease>,
        key: AttemptKey,
        replaced_existing_request: bool,
        pending: Option<AttemptJob>,
        active_stop: Option<AttemptStopSignal>,
    },
}

impl CaptureTransition {
    fn apply(self) -> Arc<CompletionRequestLease> {
        match self {
            Self::Duplicate { request, key } => {
                tracing::trace!(
                    path = %key.session.path.display(),
                    session = key.session.session.get(),
                    revision = key.document_revision.get(),
                    line = key.line,
                    character = key.character,
                    "captured duplicate logical completion request"
                );
                request
            }
            Self::New {
                request,
                key,
                replaced_existing_request,
                pending,
                active_stop,
            } => {
                tracing::trace!(
                    request = request.id.0,
                    path = %key.session.path.display(),
                    session = key.session.session.get(),
                    revision = key.document_revision.get(),
                    line = key.line,
                    character = key.character,
                    replaced_existing_request,
                    "captured logical completion request"
                );
                if let Some(pending) = pending {
                    pending.complete(CompletionAttemptOutcome::Replaced);
                }
                if let Some(active_stop) = active_stop {
                    active_stop.send(AttemptStopReason::Replaced);
                }
                request
            }
        }
    }
}

/// The newest logical request used to recognize exact duplicates.
struct CurrentRequest {
    id: RequestId,
    initial: AttemptKey,
    lease: Weak<CompletionRequestLease>,
}

/// A session is either idle or has exactly one active attempt.
///
/// While an attempt is active, only the latest newer attempt is retained. Encoding these choices
/// in one enum avoids independent fields that could describe impossible queue states.
#[derive(Default)]
enum QueueWork {
    #[default]
    Idle,
    Running {
        active: ActiveAttempt,
        latest_pending: Option<AttemptJob>,
    },
}

/// State for one open document session.
#[derive(Default)]
struct SessionQueue {
    current: Option<CurrentRequest>,
    work: QueueWork,
}

impl SessionQueue {
    /// Share exact work, start from idle, or replace the one pending position.
    fn schedule(&mut self, job: AttemptJob) -> ScheduleAction {
        match &mut self.work {
            QueueWork::Idle => {
                let (active, running) = job.start();
                self.work = QueueWork::Running {
                    active,
                    latest_pending: None,
                };
                ScheduleAction::Start(running)
            }
            QueueWork::Running {
                active,
                latest_pending,
            } => {
                if active.identity() == job.identity() && active.has_waiters() {
                    return ScheduleAction::ShareActive(active.subscribe());
                }
                if let Some(pending) = latest_pending
                    && pending.identity() == job.identity()
                {
                    return ScheduleAction::SharePending(pending.subscribe());
                }

                let reason = if active.identity().request == job.identity().request {
                    AttemptStopReason::DocumentAdvanced
                } else {
                    AttemptStopReason::Replaced
                };
                let replaced = latest_pending.replace(job);
                ScheduleAction::Queue {
                    replaced,
                    stop: active.stop_signal(),
                    reason,
                }
            }
        }
    }

    fn finish_attempt(&mut self, identity: &AttemptIdentity) -> Option<RunningAttempt> {
        let work = std::mem::replace(&mut self.work, QueueWork::Idle);
        let QueueWork::Running {
            active,
            latest_pending,
        } = work
        else {
            panic!("completion queue should be running while an attempt finishes");
        };
        assert_eq!(
            active.identity(),
            identity,
            "finished completion attempt should be the active attempt"
        );

        let job = latest_pending?;
        let (active, running) = job.start();
        self.work = QueueWork::Running {
            active,
            latest_pending: None,
        };
        Some(running)
    }

    /// Detach work that belonged to the previous logical request.
    fn replace_request_work(&mut self) -> (Option<AttemptJob>, Option<AttemptStopSignal>) {
        match &mut self.work {
            QueueWork::Idle => (None, None),
            QueueWork::Running {
                active,
                latest_pending,
            } => (latest_pending.take(), Some(active.stop_signal())),
        }
    }
}

/// Queue decision made under the mutex and carried out after the mutex is released.
enum ScheduleAction {
    ShareActive(AttemptWaiter),
    SharePending(AttemptWaiter),
    Start(RunningAttempt),
    Queue {
        replaced: Option<AttemptJob>,
        stop: AttemptStopSignal,
        reason: AttemptStopReason,
    },
}
