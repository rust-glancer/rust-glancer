//! Editor state frozen by the server before it sends a request to an analysis engine.
//!
//! The server owns the live document. The engine receives text and ids captured at one moment, not
//! a handle that can start returning newer text halfway through a query. Semantic requests receive
//! all applicable open Rust documents, including requests that return results in just one file.
//!
//! An editor snapshot has two paths for different jobs. `target.path` is the path used by the
//! editor session. `source_path` is the path by which the selected project knows the same file.
//! Routing chooses `source_path` once, so analysis does not ask the filesystem to rediscover it
//! after a rename or removal.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gen_lsp_types::{Position, Range};
use serde::{Deserialize, Serialize};

/// Identifies one period during which a path stays open in the editor.
///
/// Closing and reopening the same path creates a new session even if the client reuses a document
/// version number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpenDocumentSession(u64);

impl OpenDocumentSession {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identifies one full-text value accepted during an open document session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocumentRevision(u64);

impl DocumentRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identifies one captured state of all open documents used by semantic operations.
///
/// Opening, closing, editing, or finishing the route for any open document advances this number.
/// A semantic result can be published only while the number is still unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OpenDocumentsRevision(u64);

impl OpenDocumentsRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Small identity returned with a result so the server can check the target is still current.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetDocumentRevision {
    path: PathBuf,
    session: OpenDocumentSession,
    revision: DocumentRevision,
}

impl TargetDocumentRevision {
    pub fn new(path: PathBuf, session: OpenDocumentSession, revision: DocumentRevision) -> Self {
        Self {
            path,
            session,
            revision,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn session(&self) -> OpenDocumentSession {
        self.session
    }

    pub const fn revision(&self) -> DocumentRevision {
        self.revision
    }
}

/// Text and identity of one open Rust document when a request started.
///
/// Syntax-only requests can use this alone. Semantic requests select it within an
/// [`EditorSnapshot`], so declarations in other edited files are effective too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorDocumentSnapshot {
    target: TargetDocumentRevision,
    /// Source identity selected by routing for this document.
    ///
    /// `target.path` is still used to check the editor session. This path is used for project
    /// lookup, and was chosen by routing while the file identity was known.
    source_path: PathBuf,
    client_version: Option<i32>,
    text: Arc<str>,
}

impl EditorDocumentSnapshot {
    pub fn new(
        path: PathBuf,
        session: OpenDocumentSession,
        revision: DocumentRevision,
        client_version: Option<i32>,
        text: String,
    ) -> Self {
        let source_path = path.clone();
        Self {
            target: TargetDocumentRevision::new(path, session, revision),
            source_path,
            client_version,
            text: text.into(),
        }
    }

    /// Attach the path selected for this document by its engine route.
    pub fn with_source_path(mut self, source_path: PathBuf) -> Self {
        self.source_path = source_path;
        self
    }

    pub fn target(&self) -> &TargetDocumentRevision {
        &self.target
    }

    pub fn path(&self) -> &Path {
        self.target.path()
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub const fn session(&self) -> OpenDocumentSession {
        self.target.session()
    }

    pub const fn revision(&self) -> DocumentRevision {
        self.target.revision()
    }

    pub const fn client_version(&self) -> Option<i32> {
        self.client_version
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
}

/// Complete applicable editor inputs, captured together before dispatch.
///
/// Workspace requests use this directly. Document requests select a document within it; both
/// kinds depend on the same revision, because changing a sibling can change their answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSnapshot {
    revision: OpenDocumentsRevision,
    documents: Vec<EditorDocumentSnapshot>,
}

impl EditorSnapshot {
    pub fn new(
        revision: OpenDocumentsRevision,
        mut documents: Vec<EditorDocumentSnapshot>,
    ) -> Self {
        documents.sort_by(|a, b| a.path().cmp(b.path()));
        Self {
            revision,
            documents,
        }
    }

    pub fn revision(&self) -> OpenDocumentsRevision {
        self.revision
    }

    pub fn documents(&self) -> &[EditorDocumentSnapshot] {
        &self.documents
    }

    pub fn document_for_source(&self, path: &Path) -> Option<&EditorDocumentSnapshot> {
        self.documents
            .iter()
            .find(|document| document.source_path() == path)
    }

    pub fn select(self, target: &TargetDocumentRevision) -> Option<DocumentSnapshot> {
        let index = self
            .documents
            .iter()
            .position(|document| document.target() == target)?;
        Some(DocumentSnapshot {
            editor: self,
            index,
        })
    }
}

/// A selected document in a complete editor capture.
///
/// Selection is checked at construction and on deserialization. Keeping the target in the capture
/// prevents a request from analyzing one text while translating positions in another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DocumentSelection")]
pub struct DocumentSnapshot {
    editor: EditorSnapshot,
    index: usize,
}

impl TryFrom<DocumentSelection> for DocumentSnapshot {
    type Error = &'static str;

    fn try_from(value: DocumentSelection) -> Result<Self, Self::Error> {
        if value.index >= value.editor.documents.len() {
            return Err("selected document is absent from the editor capture");
        }
        Ok(Self {
            editor: value.editor,
            index: value.index,
        })
    }
}

impl DocumentSnapshot {
    pub fn editor(&self) -> &EditorSnapshot {
        &self.editor
    }

    pub fn document(&self) -> &EditorDocumentSnapshot {
        &self.editor.documents[self.index]
    }

    pub fn with_position(self, position: Position) -> DocumentPositionSnapshot {
        DocumentPositionSnapshot {
            snapshot: self,
            position,
        }
    }

    pub fn with_range(self, range: Range) -> DocumentRangeSnapshot {
        DocumentRangeSnapshot {
            snapshot: self,
            range,
        }
    }
}

#[derive(Deserialize)]
struct DocumentSelection {
    editor: EditorSnapshot,
    index: usize,
}

/// Cursor in a selected document, interpreted using the complete captured editor state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPositionSnapshot {
    snapshot: DocumentSnapshot,
    position: Position,
}

impl DocumentPositionSnapshot {
    pub fn snapshot(&self) -> &DocumentSnapshot {
        &self.snapshot
    }

    pub fn document(&self) -> &EditorDocumentSnapshot {
        self.snapshot.document()
    }

    pub fn position(&self) -> Position {
        self.position
    }

    pub fn into_parts(self) -> (DocumentSnapshot, Position) {
        (self.snapshot, self.position)
    }
}

/// Range in a selected document, interpreted using the complete captured editor state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRangeSnapshot {
    snapshot: DocumentSnapshot,
    range: Range,
}

impl DocumentRangeSnapshot {
    pub fn snapshot(&self) -> &DocumentSnapshot {
        &self.snapshot
    }

    pub fn document(&self) -> &EditorDocumentSnapshot {
        self.snapshot.document()
    }

    pub fn range(&self) -> Range {
        self.range
    }

    pub fn into_parts(self) -> (DocumentSnapshot, Range) {
        (self.snapshot, self.range)
    }
}
