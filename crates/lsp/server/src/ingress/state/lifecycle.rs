//! Async work left after open/save/close has been prepared in incoming message order.
//!
//! Before an LSP handler starts, `EditorState` records a newly opened session, prepares a save, or
//! marks a session closed. Some work cannot happen there because it must await an engine or the
//! registry: finding the engine for an open document, sending saved text, or removing a closed
//! document from the registry.
//!
//! This module runs those async steps in order for each document path. For example, an open must
//! finish finding its engine before a save can use that engine, and the save must finish before a
//! close removes the route. A slow step for one path does not block unrelated paths.
//!
//! Finishing route lookup or close cleanup notifies `EditorState` so a later request sees the new
//! state. These lifecycle values do not provide another way to read or change live document text,
//! sessions, or revisions.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use rg_lsp_proto::{EditorDocumentSnapshot, OpenDocumentSession, SaveProposal};
use tokio::sync::watch;

use crate::{engine_client::EngineClient, engine_registry::OpenDocumentRoute};

use super::EditorState;

/// Async work assigned after the matching open/save/close message was prepared.
#[derive(Clone, Debug)]
pub(crate) enum LifecycleEvent {
    /// Find the analysis engine for an already-recorded open session and store that route.
    Open {
        document: Arc<EditorDocumentSnapshot>,
        route: SessionRoute,
    },
    /// Send the accepted saved text to the route assigned to this open session.
    Save {
        proposal: SaveProposal,
        route: SessionRoute,
    },
    /// Remove the registry route after earlier async work for this path has finished.
    Close { path: PathBuf },
}

/// One async lifecycle step together with the earlier step it must wait for.
///
/// Before this value is created, open has recorded its session and text, save has captured its full
/// text, or close has marked its session closed. This value orders only the remaining async work:
/// open must finish before save uses its route, and save must finish before close removes it. Each
/// path has its own sequence.
#[derive(Clone, Debug)]
pub(crate) struct SequencedLifecycleEvent {
    previous: LifecycleBarrier,
    event: Arc<LifecycleEvent>,
    completion: LifecycleCompletion,
}

impl SequencedLifecycleEvent {
    pub(super) fn new(
        previous: LifecycleBarrier,
        event: LifecycleEvent,
        cleanup: Option<ClosedDocumentCleanup>,
    ) -> (Self, LifecycleBarrier) {
        let (completion, current) = LifecycleCompletion::new(cleanup);
        (
            Self {
                previous,
                event: Arc::new(event),
                completion,
            },
            current,
        )
    }

    pub(crate) async fn wait_for_previous(&self) {
        self.previous.clone().wait().await;
    }

    pub(crate) fn event(&self) -> LifecycleEvent {
        self.event.as_ref().clone()
    }

    pub(crate) fn finish(&self) {
        self.completion.finish();
    }
}

/// Result of finding the analysis engine for one open editor session.
///
/// The lookup starts when the document opens. A request may take its snapshot before the result is
/// ready; that snapshot reports the route as unavailable rather than choosing another engine.
/// When the result is stored, the global editor revision advances so the next request can include
/// the document in the correct engine input. The same result is reused until the session closes.
#[derive(Clone, Debug)]
pub(crate) struct SessionRoute {
    state: Arc<Mutex<SessionRouteState>>,
    publication: Arc<RoutePublication>,
}

impl SessionRoute {
    pub(super) fn resolving(
        editor: Weak<Mutex<EditorState>>,
        path: PathBuf,
        session: OpenDocumentSession,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionRouteState::Resolving)),
            publication: Arc::new(RoutePublication {
                editor,
                path,
                session,
            }),
        }
    }

    pub(crate) fn publish(&self, route: anyhow::Result<Option<OpenDocumentRoute>>) {
        let resolved = match route {
            Ok(Some(route)) => SessionRouteState::Ready(route),
            Ok(None) => SessionRouteState::Unavailable(Arc::from(
                "the open document could not be associated with an analysis engine",
            )),
            Err(error) => SessionRouteState::Unavailable(Arc::from(format!(
                "the document analysis route is unavailable: {error:#}"
            ))),
        };

        let mut state = self
            .state
            .lock()
            .expect("editor session route mutex should not be poisoned");
        if !matches!(*state, SessionRouteState::Resolving) {
            tracing::debug!("ignored duplicate editor session route publication");
            return;
        }
        *state = resolved;
        drop(state);
        self.publication.finish();
    }

    pub(crate) fn engine_client(&self) -> Result<EngineClient, Arc<str>> {
        self.analysis_route()
            .map(|route| route.engine_client().clone())
    }

    pub(super) fn analysis_route(&self) -> Result<OpenDocumentRoute, Arc<str>> {
        match &*self
            .state
            .lock()
            .expect("editor session route mutex should not be poisoned")
        {
            SessionRouteState::Resolving => Err(Arc::from(
                "the document analysis route is still being resolved",
            )),
            SessionRouteState::Ready(route) => Ok(route.clone()),
            SessionRouteState::Unavailable(reason) => Err(Arc::clone(reason)),
        }
    }
}

/// Notifies `EditorState` when route lookup finishes for an open session.
///
/// The notification advances the global editor revision only if this is still the same open
/// session. A route that finishes after close or reopen must not change the newer session.
#[derive(Debug)]
struct RoutePublication {
    editor: Weak<Mutex<EditorState>>,
    path: PathBuf,
    session: OpenDocumentSession,
}

impl RoutePublication {
    fn finish(&self) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        editor
            .lock()
            .expect("editor state mutex should not be poisoned")
            .route_published(&self.path, self.session);
    }
}

/// Progress and final result of finding a route for one open session.
#[derive(Debug)]
enum SessionRouteState {
    Resolving,
    Ready(OpenDocumentRoute),
    Unavailable(Arc<str>),
}

/// Wait handle that becomes ready when the preceding async step for this path finishes.
#[derive(Clone, Debug)]
pub(super) struct LifecycleBarrier {
    completed: watch::Receiver<bool>,
}

impl LifecycleBarrier {
    pub(super) fn completed() -> Self {
        let (_, completed) = watch::channel(true);
        Self { completed }
    }

    async fn wait(mut self) {
        while !*self.completed.borrow_and_update() {
            if self.completed.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Marks one async step finished and releases the next step exactly once.
#[derive(Clone, Debug)]
struct LifecycleCompletion {
    state: Arc<LifecycleCompletionState>,
}

impl LifecycleCompletion {
    fn new(cleanup: Option<ClosedDocumentCleanup>) -> (Self, LifecycleBarrier) {
        let (completed, receiver) = watch::channel(false);
        (
            Self {
                state: Arc::new(LifecycleCompletionState {
                    completed,
                    finished: AtomicBool::new(false),
                    cleanup,
                }),
            },
            LifecycleBarrier {
                completed: receiver,
            },
        )
    }

    fn finish(&self) {
        self.state.finish();
    }
}

#[derive(Debug)]
struct LifecycleCompletionState {
    completed: watch::Sender<bool>,
    finished: AtomicBool,
    cleanup: Option<ClosedDocumentCleanup>,
}

impl LifecycleCompletionState {
    fn finish(&self) {
        if self.finished.swap(true, Ordering::Relaxed) {
            return;
        }
        self.completed.send_replace(true);
        if let Some(cleanup) = &self.cleanup {
            cleanup.finish();
        }
    }
}

impl Drop for LifecycleCompletionState {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Removes a closed path from `EditorState` after its final async step has finished.
#[derive(Debug)]
pub(super) struct ClosedDocumentCleanup {
    state: Weak<Mutex<EditorState>>,
    path: PathBuf,
    session: OpenDocumentSession,
}

impl ClosedDocumentCleanup {
    pub(super) fn new(
        state: Weak<Mutex<EditorState>>,
        path: PathBuf,
        session: OpenDocumentSession,
    ) -> Self {
        Self {
            state,
            path,
            session,
        }
    }

    fn finish(&self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .remove_completed_close(&self.path, self.session);
    }
}
