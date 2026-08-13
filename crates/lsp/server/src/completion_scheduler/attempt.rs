//! One completion engine attempt and the consumers that share its result.
//!
//! The session queue decides which attempt may run. This module owns the mechanics of running that
//! attempt: its exact snapshot identity, engine future, editor-invalidation signal, interruption
//! signal, and shared result channel.

use std::sync::Arc;

use rg_lsp_proto::{
    AnalysisOutcome, DocumentPositionSnapshot, DocumentRevision, EditorSnapshotRevision,
    TargetDocumentRevision,
};
use tokio::sync::watch;
use tower_lsp_server::ls_types::{CompletionItem, Position};

use super::{
    request::{CompletionAttemptOutcome, CompletionFuture, CompletionResult},
    session::{RequestId, SessionKey},
};
use crate::ingress::{CapturedDocument, EditorRevisionWatch};

/// Identity of one engine attempt for an exact editor snapshot and cursor.
///
/// Matching only the text or cursor is insufficient: work may be shared only when the open
/// session, document revision, global editor revision, and cursor all match.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct AttemptKey {
    pub(super) session: SessionKey,
    pub(super) document_revision: DocumentRevision,
    pub(super) editor_revision: EditorSnapshotRevision,
    pub(super) line: u32,
    pub(super) character: u32,
}

impl AttemptKey {
    pub(super) fn for_position(input: &DocumentPositionSnapshot) -> Self {
        Self::new(
            input.analysis.target().clone(),
            input.analysis.editor().revision(),
            input.position,
        )
    }

    pub(super) fn for_capture(captured: &CapturedDocument, position: Position) -> Self {
        Self::new(
            captured.document().target().clone(),
            captured.editor_revision(),
            position,
        )
    }

    fn new(
        target: TargetDocumentRevision,
        editor_revision: EditorSnapshotRevision,
        position: Position,
    ) -> Self {
        Self {
            session: SessionKey {
                path: target.path().to_path_buf(),
                session: target.session(),
            },
            document_revision: target.revision(),
            editor_revision,
            line: position.line,
            character: position.character,
        }
    }
}

/// Links an exact snapshot and cursor to the logical request allowed to own that work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AttemptIdentity {
    pub(super) request: RequestId,
    pub(super) key: AttemptKey,
}

/// One accepted engine attempt, including the future that has not yet completed.
pub(super) struct AttemptJob {
    identity: AttemptIdentity,
    invalidation: EditorRevisionWatch,
    run: CompletionFuture,
    result: AttemptResultPublisher,
}

impl AttemptJob {
    pub(super) fn new(
        request: RequestId,
        key: AttemptKey,
        invalidation: EditorRevisionWatch,
        run: CompletionFuture,
    ) -> (Self, AttemptWaiter) {
        let (result, waiter) = AttemptResultPublisher::channel();
        (
            Self {
                identity: AttemptIdentity { request, key },
                invalidation,
                run,
                result,
            },
            waiter,
        )
    }

    pub(super) fn start(self) -> (ActiveAttempt, RunningAttempt) {
        let (stop, stopped) = AttemptStopSignal::channel();
        let active = ActiveAttempt {
            identity: self.identity.clone(),
            result: self.result.clone(),
            stop,
        };
        (active, RunningAttempt { job: self, stopped })
    }

    pub(super) fn identity(&self) -> &AttemptIdentity {
        &self.identity
    }

    fn result_publisher(&self) -> &AttemptResultPublisher {
        &self.result
    }

    pub(super) fn subscribe(&self) -> AttemptWaiter {
        self.result.subscribe()
    }

    pub(super) fn complete(&self, outcome: SharedAttemptOutcome) {
        self.result.complete(outcome);
    }
}

/// The small projection of a running job that remains visible to later submissions.
pub(super) struct ActiveAttempt {
    identity: AttemptIdentity,
    result: AttemptResultPublisher,
    stop: AttemptStopSignal,
}

impl ActiveAttempt {
    pub(super) fn identity(&self) -> &AttemptIdentity {
        &self.identity
    }

    pub(super) fn has_waiters(&self) -> bool {
        self.result.has_waiters()
    }

    pub(super) fn subscribe(&self) -> AttemptWaiter {
        self.result.subscribe()
    }

    pub(super) fn stop_signal(&self) -> AttemptStopSignal {
        self.stop.clone()
    }
}

/// The worker-owned attempt that is being polled.
pub(super) struct RunningAttempt {
    job: AttemptJob,
    stopped: watch::Receiver<Option<AttemptStopReason>>,
}

impl RunningAttempt {
    pub(super) fn identity(&self) -> &AttemptIdentity {
        self.job.identity()
    }

    pub(super) fn result_publisher(&self) -> &AttemptResultPublisher {
        self.job.result_publisher()
    }

    /// Wait for the first reason this attempt should stop being active.
    pub(super) async fn run(&mut self) -> Option<SharedAttemptOutcome> {
        tracing::trace!(
            request = self.job.identity.request.0,
            path = %self.job.identity.key.session.path.display(),
            session = self.job.identity.key.session.session.get(),
            revision = self.job.identity.key.document_revision.get(),
            editor_revision = self.job.identity.key.editor_revision.get(),
            line = self.job.identity.key.line,
            character = self.job.identity.key.character,
            "started completion semantic attempt"
        );

        tokio::select! {
            biased;
            _ = self.job.result.closed() => {
                tracing::trace!(
                    path = %self.job.identity.key.session.path.display(),
                    session = self.job.identity.key.session.session.get(),
                    revision = self.job.identity.key.document_revision.get(),
                    editor_revision = self.job.identity.key.editor_revision.get(),
                    line = self.job.identity.key.line,
                    character = self.job.identity.key.character,
                    "cancelled completion after every coalesced consumer closed"
                );
                None
            }
            reason = async {
                // The watch channel retains the latest reason. If a request is replaced after an
                // edit already stopped its attempt, `Replaced` remains visible to the worker.
                loop {
                    if let Some(reason) = *self.stopped.borrow_and_update() {
                        break reason;
                    }
                    if self.stopped.changed().await.is_err() {
                        break AttemptStopReason::Replaced;
                    }
                }
            } => {
                tracing::trace!(
                    path = %self.job.identity.key.session.path.display(),
                    session = self.job.identity.key.session.session.get(),
                    revision = self.job.identity.key.document_revision.get(),
                    editor_revision = self.job.identity.key.editor_revision.get(),
                    line = self.job.identity.key.line,
                    character = self.job.identity.key.character,
                    ?reason,
                    "stopped completion semantic attempt"
                );
                Some(reason.into())
            }
            _ = self.job.invalidation.superseded() => {
                tracing::debug!(
                    path = %self.job.identity.key.session.path.display(),
                    session = self.job.identity.key.session.session.get(),
                    revision = self.job.identity.key.document_revision.get(),
                    editor_revision = self.job.identity.key.editor_revision.get(),
                    current_editor_revision = self.job.invalidation.current_revision().get(),
                    line = self.job.identity.key.line,
                    character = self.job.identity.key.character,
                    "cancelled semantic attempt after editor advancement"
                );
                Some(SharedAttemptOutcome::EditorAdvanced)
            }
            result = self.job.run.as_mut() => {
                tracing::trace!(
                    path = %self.job.identity.key.session.path.display(),
                    session = self.job.identity.key.session.session.get(),
                    revision = self.job.identity.key.document_revision.get(),
                    editor_revision = self.job.identity.key.editor_revision.get(),
                    line = self.job.identity.key.line,
                    character = self.job.identity.key.character,
                    failed = result.is_err(),
                    "completed scheduled completion semantic attempt"
                );
                Some(SharedAttemptOutcome::from_result(result))
            },
        }
    }
}

/// Why the handler should stop waiting for this particular engine attempt.
#[derive(Clone, Copy, Debug)]
pub(super) enum AttemptStopReason {
    EditorAdvanced,
    Replaced,
}

/// Queue-to-worker signal that stops an active attempt for a newer piece of work.
#[derive(Clone)]
pub(super) struct AttemptStopSignal {
    sender: watch::Sender<Option<AttemptStopReason>>,
}

impl AttemptStopSignal {
    fn channel() -> (Self, watch::Receiver<Option<AttemptStopReason>>) {
        let (sender, receiver) = watch::channel(None);
        (Self { sender }, receiver)
    }

    pub(super) fn send(&self, reason: AttemptStopReason) {
        self.sender.send_replace(Some(reason));
    }
}

/// Clonable value placed in the shared result channel.
///
/// Engine errors become shared text here because `anyhow::Error` itself cannot be cloned.
#[derive(Clone, Debug)]
pub(super) enum SharedAttemptOutcome {
    Outcome(AnalysisOutcome<Vec<CompletionItem>>),
    Failed(Arc<str>),
    EditorAdvanced,
    Replaced,
}

impl SharedAttemptOutcome {
    fn from_result(result: CompletionResult) -> Self {
        match result {
            Ok(outcome) => Self::Outcome(outcome),
            Err(error) => Self::Failed(Arc::from(format!("{error:#}"))),
        }
    }
}

impl From<AttemptStopReason> for SharedAttemptOutcome {
    fn from(reason: AttemptStopReason) -> Self {
        match reason {
            AttemptStopReason::EditorAdvanced => Self::EditorAdvanced,
            AttemptStopReason::Replaced => Self::Replaced,
        }
    }
}

impl From<SharedAttemptOutcome> for CompletionAttemptOutcome {
    fn from(outcome: SharedAttemptOutcome) -> Self {
        match outcome {
            SharedAttemptOutcome::Outcome(outcome) => Self::Completed(Ok(outcome)),
            SharedAttemptOutcome::Failed(error) => {
                Self::Completed(Err(anyhow::anyhow!(error.to_string())))
            }
            SharedAttemptOutcome::EditorAdvanced => Self::EditorAdvanced,
            SharedAttemptOutcome::Replaced => Self::Replaced,
        }
    }
}

/// Publisher kept by the queue and worker while duplicate consumers wait for one result.
#[derive(Clone)]
pub(super) struct AttemptResultPublisher {
    sender: watch::Sender<Option<SharedAttemptOutcome>>,
}

impl AttemptResultPublisher {
    fn channel() -> (Self, AttemptWaiter) {
        let (sender, receiver) = watch::channel(None);
        (Self { sender }, AttemptWaiter { receiver })
    }

    fn subscribe(&self) -> AttemptWaiter {
        AttemptWaiter {
            receiver: self.sender.subscribe(),
        }
    }

    fn has_waiters(&self) -> bool {
        self.sender.receiver_count() > 0
    }

    async fn closed(&self) {
        self.sender.closed().await;
    }

    pub(super) fn complete(&self, outcome: SharedAttemptOutcome) {
        self.sender.send_replace(Some(outcome));
    }
}

/// One consumer of a shared attempt result.
///
/// Dropping every waiter lets the worker stop polling engine work that nobody can use.
pub(super) struct AttemptWaiter {
    receiver: watch::Receiver<Option<SharedAttemptOutcome>>,
}

impl AttemptWaiter {
    pub(super) fn ready(outcome: SharedAttemptOutcome) -> Self {
        let (result, waiter) = AttemptResultPublisher::channel();
        result.complete(outcome);
        waiter
    }

    pub(super) async fn wait(mut self) -> CompletionAttemptOutcome {
        loop {
            if let Some(result) = self.receiver.borrow_and_update().clone() {
                return result.into();
            }
            if self.receiver.changed().await.is_err() {
                return CompletionAttemptOutcome::Completed(Err(anyhow::anyhow!(
                    "scheduled completion result sender dropped before responding"
                )));
            }
        }
    }
}
