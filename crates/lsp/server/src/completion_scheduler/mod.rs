//! Keeps completion engine work bounded while the editor is changing quickly.
//!
//! Ordered ingress creates a [`CompletionRequest`] for every completion message before its async
//! handler starts. The request then submits one engine attempt for each immutable editor snapshot
//! that the handler tries. A newer completion message replaces the whole request; an edit may
//! instead let the same request move its cursor and try a newer snapshot.
//!
//! Requests compete only inside one open document session. That session is either idle or running
//! one attempt with, at most, the latest newer attempt waiting behind it. Exact duplicate requests
//! and attempts share their result instead of starting duplicate engine work.
//!
//! This module does not own editor text, revisions, or retry policy. `EditorState` remains the
//! source of editor state, and the completion method decides whether to recapture after an attempt
//! was overtaken. The scheduler only owns logical request identity and bounded engine work.

use std::sync::{Arc, Mutex};

use tower_lsp_server::ls_types::Position;

use crate::ingress::CapturedDocument;

mod attempt;
mod request;
mod session;

pub(crate) use self::request::{CompletionAttemptOutcome, CompletionRequest};

/// Entry point used by ordered ingress to register completion messages.
///
/// Engine attempts are submitted through the returned [`CompletionRequest`]. Keeping submission
/// on the request makes it impossible for a handler to schedule work without the logical request
/// identity established at ingress.
#[derive(Clone, Default)]
pub(crate) struct CompletionScheduler {
    state: Arc<Mutex<session::SchedulerState>>,
}

impl CompletionScheduler {
    /// Register one completion message before its async handler starts.
    ///
    /// Registration therefore follows wire order even when handler futures are later polled in a
    /// different order. An exact duplicate shares the existing logical request.
    pub(crate) fn capture_request(
        &self,
        captured: &CapturedDocument,
        position: Position,
    ) -> CompletionRequest {
        CompletionRequest::capture(&self.state, captured, position)
    }

    #[cfg(test)]
    fn session_count(&self) -> usize {
        self.state
            .lock()
            .expect("completion scheduler mutex should not be poisoned")
            .session_count()
    }
}

impl std::fmt::Debug for CompletionScheduler {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("CompletionScheduler")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
