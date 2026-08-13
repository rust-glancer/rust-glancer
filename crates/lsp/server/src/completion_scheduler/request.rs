//! Logical completion requests created at ordered LSP ingress.
//!
//! A request is the capability to submit engine attempts for one completion message. Exact
//! duplicate messages share its lease. The last clone releases the request's place in the
//! per-session queue, while a newer non-duplicate message marks it as replaced immediately.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use rg_lsp_proto::{AnalysisOutcome, DocumentPositionSnapshot};
use tower_lsp_server::ls_types::{CompletionItem, Position};

use super::{
    attempt::{AttemptKey, AttemptWaiter},
    session::{RequestId, SchedulerState, SessionKey},
};
use crate::ingress::{CapturedDocument, EditorRevisionWatch};

pub(super) type CompletionResult = anyhow::Result<AnalysisOutcome<Vec<CompletionItem>>>;
pub(super) type CompletionFuture = Pin<Box<dyn Future<Output = CompletionResult> + Send>>;

/// What happened to one engine query made for a still-live completion request.
#[derive(Debug)]
pub(crate) enum CompletionAttemptOutcome {
    /// The engine query finished; the handler must still validate its tagged editor input.
    Completed(CompletionResult),
    /// The editor changed, so this request may take a newer snapshot and try again.
    EditorAdvanced,
    /// A newer completion message replaced this request.
    Replaced,
}

/// Handle for one logical completion message received from the editor.
///
/// The handle owns submission because every semantic attempt must remain attached to the logical
/// request established at ingress. Clones are used only by exact duplicate messages that are
/// allowed to share engine work.
#[derive(Clone)]
pub(crate) struct CompletionRequest {
    lease: Arc<CompletionRequestLease>,
}

impl CompletionRequest {
    pub(super) fn capture(
        state: &Arc<Mutex<SchedulerState>>,
        captured: &CapturedDocument,
        position: Position,
    ) -> Self {
        let key = AttemptKey::for_capture(captured, position);
        Self {
            lease: SchedulerState::capture_request(state, key),
        }
    }

    pub(crate) fn is_replaced(&self) -> bool {
        self.lease.is_replaced()
    }

    /// Run one engine attempt for the supplied immutable document snapshot.
    ///
    /// The snapshot is passed into `run` only after its full attempt identity has been recorded.
    /// This prevents a caller from accidentally identifying one snapshot while querying another.
    pub(crate) async fn submit_attempt<Run, Fut>(
        &self,
        input: DocumentPositionSnapshot,
        invalidation: EditorRevisionWatch,
        run: Run,
    ) -> CompletionAttemptOutcome
    where
        Run: FnOnce(DocumentPositionSnapshot) -> Fut,
        Fut: Future<Output = CompletionResult> + Send + 'static,
    {
        let key = AttemptKey::for_position(&input);
        self.enqueue_attempt(key, invalidation, Box::pin(run(input)))
            .wait()
            .await
    }

    pub(super) fn enqueue_attempt(
        &self,
        key: AttemptKey,
        invalidation: EditorRevisionWatch,
        run: CompletionFuture,
    ) -> AttemptWaiter {
        SchedulerState::enqueue_attempt(&self.lease, key, invalidation, run)
    }

    #[cfg(test)]
    pub(super) fn id(&self) -> RequestId {
        self.lease.id
    }
}

impl std::fmt::Debug for CompletionRequest {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("CompletionRequest")
            .field("id", &self.lease.id)
            .field("session", &self.lease.session)
            .field("replaced", &self.is_replaced())
            .finish()
    }
}

/// State shared by every exact duplicate consumer of one logical request.
///
/// The scheduler reference is weak on purpose: request handles must not keep the global scheduler
/// alive. Dropping the last handle only releases this request; it cannot disturb a newer request
/// that has already taken ownership of the same session queue.
pub(super) struct CompletionRequestLease {
    pub(super) id: RequestId,
    pub(super) session: SessionKey,
    replaced: AtomicBool,
    scheduler: Weak<Mutex<SchedulerState>>,
}

impl CompletionRequestLease {
    pub(super) fn new(
        id: RequestId,
        session: SessionKey,
        scheduler: Weak<Mutex<SchedulerState>>,
    ) -> Self {
        Self {
            id,
            session,
            replaced: AtomicBool::new(false),
            scheduler,
        }
    }

    pub(super) fn is_replaced(&self) -> bool {
        self.replaced.load(Ordering::Acquire)
    }

    pub(super) fn mark_replaced(&self) {
        self.replaced.store(true, Ordering::Release);
    }

    pub(super) fn scheduler(&self) -> &Weak<Mutex<SchedulerState>> {
        &self.scheduler
    }
}

impl Drop for CompletionRequestLease {
    fn drop(&mut self) {
        let Some(state) = self.scheduler.upgrade() else {
            return;
        };
        state
            .lock()
            .expect("completion scheduler mutex should not be poisoned")
            .release_request(&self.session, self.id);
    }
}
