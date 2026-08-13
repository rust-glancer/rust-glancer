//! Open editor documents and the immutable snapshots used by requests.
//!
//! This is the only place in the server that changes live editor documents. For each editor path it
//! records the open session, document revision, and complete synchronized text: the full document
//! after every accepted `didChange` edit has been applied. Each open session also records whether
//! its analysis engine is still being found, is ready, or could not be found. A ready route is
//! reused until that session closes.
//!
//! Updating this state and taking a request snapshot both use the same short synchronous mutex.
//! The mutex is never held while reading the filesystem or waiting for an engine. This is what
//! ensures that a request sees either the state before an editor message or the state after it,
//! never a partly updated value.
//!
//! A document request receives its target and all open sibling documents from one global editor
//! revision. If the editor changes while the request is alive, the capture may follow recorded
//! edits to move its cursor into a newer snapshot. For example, typing `ck` after `RwLo|` moves the
//! captured cursor to `RwLock|`. These links store edits rather than old full-text values, and they
//! disappear when no request needs them.
//!
//! The `lifecycle` child module runs the async parts of open/save/close. It reports when an engine
//! route has been resolved or a close has finished, but it never stores document text, sessions,
//! or revision counters.

mod lifecycle;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use tokio::sync::watch;
use tower_lsp_server::ls_types::{Position, TextDocumentContentChangeEvent};

use rg_lsp_proto::{
    DocumentAnalysisSnapshot, DocumentRevision, EditorDocumentSnapshot, EditorSnapshot,
    EditorSnapshotRevision, OpenDocumentSession, SaveProposal, TargetDocumentRevision,
};

use crate::{engine_client::EngineClient, engine_registry::OpenDocumentRoute};

use self::lifecycle::{ClosedDocumentCleanup, LifecycleBarrier};
pub(crate) use self::lifecycle::{LifecycleEvent, SequencedLifecycleEvent, SessionRoute};
use super::edit::{AppliedDocumentChanges, DocumentChangeError, PositionTransform};

/// Snapshot of editor state selected for one document request before its handler starts.
///
/// The target document, all open sibling documents, their session routes, and the global editor
/// revision are selected together. The editor path identifies the open session;
/// `analysis_snapshot` uses each resolved route to add the corresponding project source path for
/// engine analysis. A capture may be replaced with a newer snapshot, but it never writes to
/// `EditorState` or stores another live copy of editor state.
#[derive(Clone, Debug)]
pub(crate) struct CapturedDocument {
    document: Arc<EditorDocumentSnapshot>,
    editor_revision: Arc<EditorRevisionNode>,
    route: Result<OpenDocumentRoute, Arc<str>>,
    open_documents: Arc<[CapturedOpenDocument]>,
    editor: Weak<Mutex<EditorState>>,
}

impl CapturedDocument {
    pub(crate) fn document(&self) -> &EditorDocumentSnapshot {
        &self.document
    }

    pub(crate) fn engine_client(&self) -> Result<EngineClient, Arc<str>> {
        self.route
            .as_ref()
            .map(|route| route.engine_client().clone())
            .map_err(Arc::clone)
    }

    /// Build engine input from captured documents assigned to the target document's engine.
    ///
    /// Every route was read when this request snapshot was taken. A sibling whose route was not
    /// ready is therefore absent from this engine input. When that route becomes ready, editor
    /// state advances to a new revision, and a later request can include the sibling.
    pub(crate) fn analysis_snapshot(
        &self,
        engine_client: &EngineClient,
    ) -> Result<DocumentAnalysisSnapshot, DocumentUnavailable> {
        let target_route = self.route.as_ref().map_err(|reason| {
            DocumentUnavailable::new(Some(self.document.path().to_path_buf()), Arc::clone(reason))
        })?;
        if !target_route.engine_client().same_engine(engine_client) {
            return Err(DocumentUnavailable::new(
                Some(self.document.path().to_path_buf()),
                "the selected engine does not match the captured target route",
            ));
        }

        let mut documents = Vec::new();
        let mut text_by_source_path = HashMap::<PathBuf, &str>::new();
        for captured in self.open_documents.iter() {
            let Ok(route) = &captured.route else {
                continue;
            };
            if !route.engine_client().same_engine(engine_client) || !is_rust_path(&captured.path) {
                continue;
            }
            let Some(document) = &captured.document else {
                return Err(DocumentUnavailable::new(
                    Some(captured.path.clone()),
                    "an applicable open document has no synchronized full text",
                ));
            };
            if let Some(previous_text) =
                text_by_source_path.insert(route.source_path().to_path_buf(), document.text())
                && previous_text != document.text()
            {
                return Err(DocumentUnavailable::new(
                    Some(captured.path.clone()),
                    format!(
                        "open editor paths mapped to `{}` with conflicting text",
                        route.source_path().display()
                    ),
                ));
            }
            documents.push(
                document
                    .as_ref()
                    .clone()
                    .with_source_path(route.source_path().to_path_buf()),
            );
        }

        let editor = EditorSnapshot::new(self.editor_revision.revision, documents);
        if editor.document(self.document.target()).is_none() {
            return Err(DocumentUnavailable::new(
                Some(self.document.path().to_path_buf()),
                "the target document is absent from its captured editor snapshot",
            ));
        }
        Ok(DocumentAnalysisSnapshot::new(
            self.document.target().clone(),
            editor,
        ))
    }

    /// Check whether a completed result still matches the latest target and editor revisions.
    ///
    /// Analysis may finish after another editor change. Callers publish the result only when this
    /// check confirms that both the target document and the whole editor snapshot are unchanged.
    pub(crate) fn is_current(
        &self,
        target: &TargetDocumentRevision,
        editor_revision: EditorSnapshotRevision,
    ) -> bool {
        self.editor.upgrade().is_some_and(|editor| {
            editor
                .lock()
                .expect("editor state mutex should not be poisoned")
                .is_current(target, editor_revision)
        })
    }

    pub(crate) fn editor_revision(&self) -> EditorSnapshotRevision {
        self.editor_revision.revision
    }

    /// Watch for any editor change that makes this captured snapshot old.
    ///
    /// This can stop expensive work early, but it cannot prove that a completed result is still
    /// usable. Callers must still validate that result against the live `EditorState`.
    pub(crate) fn editor_revision_watch(&self) -> EditorRevisionWatch {
        EditorRevisionWatch {
            captured: Arc::clone(&self.editor_revision),
        }
    }

    /// Take a newer snapshot and move `position` through edits made since this capture.
    ///
    /// The recorded edit links explain how a position moved without storing old document text. For
    /// example, an insertion of `ck` after `RwLo|` moves the position from column 4 to column 6.
    /// After applying those links, this method asks the live `EditorState` for a new immutable
    /// snapshot of the same open session.
    pub(crate) fn recapture_position(
        &self,
        position: Position,
    ) -> Result<(Self, Position), PositionRecaptureError> {
        let editor = self.editor.upgrade().ok_or_else(|| {
            PositionRecaptureError::Unavailable(Arc::from("the editor state no longer exists"))
        })?;
        let mut recaptured = editor
            .lock()
            .expect("editor state mutex should not be poisoned")
            .recapture_position(self, position)?;
        recaptured.0.editor = Arc::downgrade(&editor);
        Ok(recaptured)
    }
}

/// Notification that a newer global editor revision has replaced this captured snapshot.
#[derive(Clone, Debug)]
pub(crate) struct EditorRevisionWatch {
    captured: Arc<EditorRevisionNode>,
}

impl EditorRevisionWatch {
    pub(crate) fn is_superseded(&self) -> bool {
        self.captured.successor.borrow().is_some()
    }

    pub(crate) fn current_revision(&self) -> EditorSnapshotRevision {
        let mut current = Arc::clone(&self.captured);
        loop {
            let Some(step) = current.successor.borrow().clone() else {
                return current.revision;
            };
            current = Arc::clone(&step.next);
        }
    }

    pub(crate) async fn superseded(&mut self) {
        let mut successor = self.captured.successor.subscribe();
        while successor.borrow_and_update().is_none() {
            if successor.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Why a position captured for one open session cannot be moved to the newest editor snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PositionRecaptureError {
    /// The path was closed or reopened under another open-session identity.
    SessionEnded,
    /// Complete text is unavailable, or the server cannot tell where the old position moved.
    Unavailable(Arc<str>),
}

impl PositionRecaptureError {
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::SessionEnded => "the captured document session has ended",
            Self::Unavailable(reason) => reason,
        }
    }
}

/// One immutable link in the editor-revision stream.
///
/// Links point only forward. `EditorState` owns the newest node, while a live capture keeps the
/// links after its revision alive. Once the oldest capture advances or drops, obsolete links are
/// released without a request registry or retained full-text history.
struct EditorRevisionNode {
    revision: EditorSnapshotRevision,
    successor: watch::Sender<Option<Arc<EditorRevisionStep>>>,
}

impl EditorRevisionNode {
    fn new(revision: EditorSnapshotRevision) -> Self {
        let (successor, _) = watch::channel(None);
        Self {
            revision,
            successor,
        }
    }
}

impl std::fmt::Debug for EditorRevisionNode {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("EditorRevisionNode")
            .field("revision", &self.revision)
            .field("has_successor", &self.successor.borrow().is_some())
            .finish()
    }
}

/// Information needed to move from one accepted global editor revision to the next.
#[derive(Debug)]
struct EditorRevisionStep {
    next: Arc<EditorRevisionNode>,
    change: EditorChange,
}

/// Editor change data needed to move a captured position, if the target document changed.
#[derive(Debug)]
enum EditorChange {
    Document {
        path: PathBuf,
        session: OpenDocumentSession,
        position_transform: PositionTransform,
    },
    Other,
}

impl EditorChange {
    fn rebase(
        &self,
        path: &Path,
        session: OpenDocumentSession,
        position: Position,
    ) -> Result<Position, PositionRecaptureError> {
        let Self::Document {
            path: changed_path,
            session: changed_session,
            position_transform,
        } = self
        else {
            return Ok(position);
        };
        if changed_path != path || *changed_session != session {
            return Ok(position);
        }

        position_transform.rebase(position).ok_or_else(|| {
            PositionRecaptureError::Unavailable(Arc::from(
                "the captured position cannot be mapped through a full or rejected document change",
            ))
        })
    }
}

/// One open document as it appeared in the same global capture as the request target.
#[derive(Clone, Debug)]
struct CapturedOpenDocument {
    path: PathBuf,
    document: Option<Arc<EditorDocumentSnapshot>>,
    route: Result<OpenDocumentRoute, Arc<str>>,
}

/// Why a document request cannot use the server's latest synchronized editor text.
#[derive(Clone, Debug)]
pub(crate) struct DocumentUnavailable {
    path: Option<PathBuf>,
    reason: Arc<str>,
}

impl DocumentUnavailable {
    fn new(path: Option<PathBuf>, reason: impl Into<Arc<str>>) -> Self {
        Self {
            path,
            reason: reason.into(),
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

/// Whether diagnostics for saved text may replace the diagnostics already shown by the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiagnosticsPublication {
    Publish { version: Option<i32> },
    KeepVisible,
}

/// Thread-safe entry point to the server's live editor state.
///
/// Each call holds the mutex only long enough to apply one incoming editor message, take a request
/// snapshot, or check whether a result is still up to date. The mutex is never held across an
/// `.await`, and this handle cannot call an engine or fall back to filesystem text.
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorStateHandle {
    state: Arc<Mutex<EditorState>>,
}

impl EditorStateHandle {
    pub(crate) fn open(
        &self,
        path: PathBuf,
        client_version: Option<i32>,
        text: String,
    ) -> SequencedLifecycleEvent {
        let weak_state = Arc::downgrade(&self.state);
        self.state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .open(path, client_version, text, weak_state)
    }

    pub(crate) fn change(
        &self,
        path: &Path,
        client_version: Option<i32>,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<bool, DocumentChangeError> {
        self.state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .change(path, client_version, changes)
    }

    pub(crate) fn save(
        &self,
        path: &Path,
        text: Option<String>,
    ) -> Option<SequencedLifecycleEvent> {
        self.state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .save(path, text)
    }

    pub(crate) fn close(&self, path: &Path) -> Option<SequencedLifecycleEvent> {
        let weak_state = Arc::downgrade(&self.state);
        self.state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .close(path, weak_state)
    }

    pub(crate) fn document(
        &self,
        path: Option<PathBuf>,
    ) -> Result<CapturedDocument, DocumentUnavailable> {
        let mut captured = self
            .state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .document(path)?;
        captured.editor = Arc::downgrade(&self.state);
        Ok(captured)
    }

    pub(crate) fn diagnostics_publication(
        &self,
        path: &Path,
        saved_text: Option<&str>,
    ) -> DiagnosticsPublication {
        self.state
            .lock()
            .expect("editor state mutex should not be poisoned")
            .diagnostics_publication(path, saved_text)
    }
}

/// Stores every live editor session together with its complete text, revisions, and route slot.
#[derive(Debug)]
struct EditorState {
    next_session: u64,
    next_revision: u64,
    editor_revision: Arc<EditorRevisionNode>,
    documents: HashMap<PathBuf, DocumentEntry>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            next_session: 0,
            next_revision: 0,
            editor_revision: Arc::new(EditorRevisionNode::new(EditorSnapshotRevision::new(0))),
            documents: HashMap::new(),
        }
    }
}

impl EditorState {
    /// Record the new session and its text now; let the async handler find its engine later.
    fn open(
        &mut self,
        path: PathBuf,
        client_version: Option<i32>,
        text: String,
        weak_state: Weak<Mutex<Self>>,
    ) -> SequencedLifecycleEvent {
        let session = self.allocate_session();
        let revision = self.allocate_revision();
        let document = Arc::new(EditorDocumentSnapshot::new(
            path.clone(),
            session,
            revision,
            client_version,
            text,
        ));
        let route = SessionRoute::resolving(weak_state, path.clone(), session);
        let previous = self
            .documents
            .get(&path)
            .map(|entry| entry.tail.clone())
            .unwrap_or_else(LifecycleBarrier::completed);
        let (sequenced, current) = SequencedLifecycleEvent::new(
            previous,
            LifecycleEvent::Open {
                document: Arc::clone(&document),
                route: route.clone(),
            },
            None,
        );

        self.documents.insert(
            path,
            DocumentEntry {
                open: Some(OpenDocument {
                    session,
                    current: Some(document),
                    route,
                }),
                last_session: session,
                tail: current,
            },
        );
        self.advance_editor_revision(EditorChange::Other);

        sequenced
    }

    /// Apply one complete `didChange`, or mark the text unavailable if any edit is rejected.
    fn change(
        &mut self,
        path: &Path,
        client_version: Option<i32>,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<bool, DocumentChangeError> {
        if !self
            .documents
            .get(path)
            .is_some_and(|entry| entry.open.is_some())
        {
            return Ok(false);
        }

        let (session, current) = self
            .documents
            .get(path)
            .and_then(|entry| entry.open.as_ref())
            .map(|open| (open.session, open.current.clone()))
            .expect("checked open document should remain open");
        let applied = match AppliedDocumentChanges::apply(
            current.as_deref().map(EditorDocumentSnapshot::text),
            changes,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                // The client has moved on, but the rejected ranges cannot produce an exact local
                // value. Invalidate the old snapshot and wait for a later full replacement.
                self.documents
                    .get_mut(path)
                    .and_then(|entry| entry.open.as_mut())
                    .expect("checked open document should remain open")
                    .current = None;
                self.advance_editor_revision(EditorChange::Document {
                    path: path.to_path_buf(),
                    session,
                    position_transform: PositionTransform::Unavailable,
                });
                return Err(error);
            }
        };
        let revision = self.allocate_revision();
        let entry = self
            .documents
            .get_mut(path)
            .expect("open document entry should remain present");
        let open = entry
            .open
            .as_mut()
            .expect("checked open document should remain open");
        let document = Arc::new(EditorDocumentSnapshot::new(
            path.to_path_buf(),
            session,
            revision,
            client_version,
            applied.text,
        ));
        open.current = Some(document);
        self.advance_editor_revision(EditorChange::Document {
            path: path.to_path_buf(),
            session,
            position_transform: applied.position_transform,
        });
        Ok(true)
    }

    /// Prepare a save only when any supplied text agrees with the synchronized editor value.
    fn save(&mut self, path: &Path, text: Option<String>) -> Option<SequencedLifecycleEvent> {
        let entry = self.documents.get_mut(path)?;
        let open = entry.open.as_ref()?;
        let current = open.current.as_ref()?;
        let text = match text {
            Some(text) if text != current.text() => return None,
            Some(text) => text,
            None => current.text().to_string(),
        };
        let proposal = SaveProposal::new(current.target().clone(), current.client_version(), text);
        let (sequenced, current) = SequencedLifecycleEvent::new(
            entry.tail.clone(),
            LifecycleEvent::Save {
                proposal,
                route: open.route.clone(),
            },
            None,
        );
        entry.tail = current;
        Some(sequenced)
    }

    /// Mark the session closed now, but keep its ordering entry until async cleanup finishes.
    fn close(
        &mut self,
        path: &Path,
        weak_state: Weak<Mutex<Self>>,
    ) -> Option<SequencedLifecycleEvent> {
        if !self
            .documents
            .get(path)
            .is_some_and(|entry| entry.open.is_some())
        {
            return None;
        }
        let entry = self.documents.get_mut(path)?;
        let open = entry.open.take()?;
        let cleanup = ClosedDocumentCleanup::new(weak_state, path.to_path_buf(), open.session);
        let (sequenced, current) = SequencedLifecycleEvent::new(
            entry.tail.clone(),
            LifecycleEvent::Close {
                path: path.to_path_buf(),
            },
            Some(cleanup),
        );
        entry.last_session = open.session;
        entry.tail = current;
        self.advance_editor_revision(EditorChange::Other);
        Some(sequenced)
    }

    /// Take the target and every open sibling from the same global editor revision.
    fn document(&self, path: Option<PathBuf>) -> Result<CapturedDocument, DocumentUnavailable> {
        let Some(path) = path else {
            return Err(DocumentUnavailable::new(
                None,
                "the request does not target a filesystem document",
            ));
        };
        let Some(entry) = self.documents.get(&path) else {
            return Err(DocumentUnavailable::new(
                Some(path),
                "the request targets a document that is not open",
            ));
        };
        let Some(open) = &entry.open else {
            return Err(DocumentUnavailable::new(
                Some(path),
                "the request targets a closed document session",
            ));
        };
        let Some(document) = &open.current else {
            return Err(DocumentUnavailable::new(
                Some(path),
                "the open document has no exact synchronized text",
            ));
        };

        let open_documents = self
            .documents
            .iter()
            .filter_map(|(path, entry)| {
                entry.open.as_ref().map(|open| CapturedOpenDocument {
                    path: path.clone(),
                    document: open.current.clone(),
                    route: open.route.analysis_route(),
                })
            })
            .collect::<Vec<_>>();

        Ok(CapturedDocument {
            document: Arc::clone(document),
            editor_revision: Arc::clone(&self.editor_revision),
            route: open.route.analysis_route(),
            open_documents: open_documents.into(),
            editor: Weak::new(),
        })
    }

    fn recapture_position(
        &self,
        captured: &CapturedDocument,
        mut position: Position,
    ) -> Result<(CapturedDocument, Position), PositionRecaptureError> {
        let path = captured.document.path();
        let session = captured.document.session();
        let Some(open) = self
            .documents
            .get(path)
            .and_then(|entry| entry.open.as_ref())
        else {
            return Err(PositionRecaptureError::SessionEnded);
        };
        if open.session != session {
            return Err(PositionRecaptureError::SessionEnded);
        }
        if open.current.is_none() {
            return Err(PositionRecaptureError::Unavailable(Arc::from(
                "the newest editor revision has no complete synchronized text",
            )));
        }

        // Every revision link is added while holding this same mutex. Starting from a valid
        // captured revision must therefore reach the latest revision without a gap. Edits to the
        // target move its request position; sibling edits and route changes only select a newer
        // complete snapshot.
        let mut revision = Arc::clone(&captured.editor_revision);
        while !Arc::ptr_eq(&revision, &self.editor_revision) {
            let Some(step) = revision.successor.borrow().clone() else {
                return Err(PositionRecaptureError::Unavailable(Arc::from(
                    "the captured editor revision is not connected to the current revision",
                )));
            };
            position = step.change.rebase(path, session, position)?;
            revision = Arc::clone(&step.next);
        }

        let recaptured = self
            .document(Some(path.to_path_buf()))
            .map_err(|unavailable| {
                PositionRecaptureError::Unavailable(Arc::from(unavailable.reason().to_string()))
            })?;
        Ok((recaptured, position))
    }

    fn is_current(
        &self,
        target: &TargetDocumentRevision,
        editor_revision: EditorSnapshotRevision,
    ) -> bool {
        self.editor_revision.revision == editor_revision
            && self
                .documents
                .get(target.path())
                .and_then(|entry| entry.open.as_ref())
                .and_then(|open| open.current.as_ref())
                .is_some_and(|document| document.target() == target)
    }

    fn route_published(&mut self, path: &Path, session: OpenDocumentSession) {
        let belongs_to_open_session = self
            .documents
            .get(path)
            .and_then(|entry| entry.open.as_ref())
            .is_some_and(|open| open.session == session);
        if belongs_to_open_session {
            self.advance_editor_revision(EditorChange::Other);
        }
    }

    fn diagnostics_publication(
        &self,
        path: &Path,
        saved_text: Option<&str>,
    ) -> DiagnosticsPublication {
        let Some(open) = self
            .documents
            .get(path)
            .and_then(|entry| entry.open.as_ref())
        else {
            return DiagnosticsPublication::Publish { version: None };
        };
        let Some(document) = &open.current else {
            return DiagnosticsPublication::KeepVisible;
        };
        if saved_text != Some(document.text()) {
            return DiagnosticsPublication::KeepVisible;
        }

        DiagnosticsPublication::Publish {
            version: document.client_version(),
        }
    }

    fn allocate_session(&mut self) -> OpenDocumentSession {
        self.next_session = self
            .next_session
            .checked_add(1)
            .expect("editor open-session counter should not overflow");
        OpenDocumentSession::new(self.next_session)
    }

    fn allocate_revision(&mut self) -> DocumentRevision {
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .expect("editor document-revision counter should not overflow");
        DocumentRevision::new(self.next_revision)
    }

    /// Link the old revision to the new one and notify requests waiting for an editor change.
    fn advance_editor_revision(&mut self, change: EditorChange) -> EditorSnapshotRevision {
        let next_revision = self
            .editor_revision
            .revision
            .get()
            .checked_add(1)
            .expect("editor snapshot-revision counter should not overflow");
        let revision = EditorSnapshotRevision::new(next_revision);
        let next = Arc::new(EditorRevisionNode::new(revision));
        let previous = Arc::clone(&self.editor_revision);
        let replaced = previous
            .successor
            .send_replace(Some(Arc::new(EditorRevisionStep {
                next: Arc::clone(&next),
                change,
            })));
        debug_assert!(
            replaced.is_none(),
            "an editor revision must have exactly one successor"
        );
        self.editor_revision = next;
        revision
    }

    fn remove_completed_close(&mut self, path: &Path, session: OpenDocumentSession) {
        let should_remove = self
            .documents
            .get(path)
            .is_some_and(|entry| entry.open.is_none() && entry.last_session == session);
        if should_remove {
            self.documents.remove(path);
        }
    }
}

/// State kept for one path while it is open or while its close handler is still finishing.
#[derive(Debug)]
struct DocumentEntry {
    open: Option<OpenDocument>,
    last_session: OpenDocumentSession,
    tail: LifecycleBarrier,
}

/// One open editor session; `current` is absent only when complete text is unavailable.
#[derive(Debug)]
struct OpenDocument {
    session: OpenDocumentSession,
    current: Option<Arc<EditorDocumentSnapshot>>,
    route: SessionRoute,
}

fn is_rust_path(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs")
}

#[cfg(test)]
mod tests;
