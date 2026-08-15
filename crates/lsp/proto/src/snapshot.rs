//! Editor state frozen by the server before it sends a request to an analysis engine.
//!
//! The server owns the live document. The engine receives text and ids captured at one moment, not
//! a handle that can start returning newer text halfway through a query. Most document reads need
//! one document snapshot. Cross-file operations also receive the other open Rust documents needed
//! to validate any saved locations or edits they actually return.
//!
//! An editor snapshot has two paths for different jobs. `target.path` is the path used by the
//! editor session. `source_path` is the path by which the selected project knows the same file.
//! Routing chooses `source_path` once, so analysis does not ask the filesystem to rediscover it
//! after a rename or removal.

use std::path::{Path, PathBuf};

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

/// Identifies one captured state of all open documents used by global operations.
///
/// Opening, closing, editing, or finishing the route for any open document advances this number.
/// A global result can be published only while the number is still unchanged.
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
/// This value tracks only its own document. A document-local query can therefore ignore edits in
/// other files. A cross-file operation places it in [`GlobalPositionSnapshot`], which also tracks
/// the complete open-document state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorDocumentSnapshot {
    target: TargetDocumentRevision,
    /// Path by which the selected saved project knows this document.
    ///
    /// `target.path` is still used to check the editor session. This path is used for project
    /// lookup, and was chosen by routing while the file identity was known.
    source_path: PathBuf,
    client_version: Option<i32>,
    text: String,
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
            text,
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

    /// Add a position to an already captured document.
    pub fn with_position(self, position: ls_types::Position) -> DocumentPositionSnapshot {
        DocumentPositionSnapshot::new(self, position)
    }

    /// Add a range to an already captured document.
    pub fn with_range(self, range: ls_types::Range) -> DocumentRangeSnapshot {
        DocumentRangeSnapshot::new(self, range)
    }
}

/// Input for a cross-file operation: the target position and all applicable open Rust documents.
///
/// Cross-file operations return locations or edits that may depend on several editor documents.
/// Operations which require a fully saved workspace compare every document before running.
/// Definition queries instead validate only documents that receive a target and may omit a target
/// that cannot be mapped safely. A successful result carries `open_documents_revision` back to the
/// server, which catches another edit that arrived while analysis was running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalPositionSnapshot {
    target: TargetDocumentRevision,
    open_documents_revision: OpenDocumentsRevision,
    documents: Vec<EditorDocumentSnapshot>,
    position: ls_types::Position,
}

impl GlobalPositionSnapshot {
    pub fn new(
        target: TargetDocumentRevision,
        open_documents_revision: OpenDocumentsRevision,
        mut documents: Vec<EditorDocumentSnapshot>,
        position: ls_types::Position,
    ) -> Self {
        documents.sort_by(|left, right| left.path().cmp(right.path()));
        Self {
            target,
            open_documents_revision,
            documents,
            position,
        }
    }

    pub fn target(&self) -> &TargetDocumentRevision {
        &self.target
    }

    pub const fn open_documents_revision(&self) -> OpenDocumentsRevision {
        self.open_documents_revision
    }

    pub fn documents(&self) -> &[EditorDocumentSnapshot] {
        &self.documents
    }

    pub fn target_document(&self) -> Option<&EditorDocumentSnapshot> {
        self.documents
            .iter()
            .find(|document| document.target() == &self.target)
    }

    pub const fn position(&self) -> ls_types::Position {
        self.position
    }

    pub fn into_parts(
        self,
    ) -> (
        TargetDocumentRevision,
        OpenDocumentsRevision,
        Vec<EditorDocumentSnapshot>,
        ls_types::Position,
    ) {
        (
            self.target,
            self.open_documents_revision,
            self.documents,
            self.position,
        )
    }
}

/// One captured document and the position to query inside that same text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPositionSnapshot {
    document: EditorDocumentSnapshot,
    position: ls_types::Position,
}

impl DocumentPositionSnapshot {
    pub fn new(document: EditorDocumentSnapshot, position: ls_types::Position) -> Self {
        Self { document, position }
    }

    pub fn document(&self) -> &EditorDocumentSnapshot {
        &self.document
    }

    pub const fn position(&self) -> ls_types::Position {
        self.position
    }

    pub fn into_parts(self) -> (EditorDocumentSnapshot, ls_types::Position) {
        (self.document, self.position)
    }
}

/// One captured document and the range to query inside that same text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRangeSnapshot {
    document: EditorDocumentSnapshot,
    range: ls_types::Range,
}

impl DocumentRangeSnapshot {
    pub fn new(document: EditorDocumentSnapshot, range: ls_types::Range) -> Self {
        Self { document, range }
    }

    pub fn document(&self) -> &EditorDocumentSnapshot {
        &self.document
    }

    pub const fn range(&self) -> ls_types::Range {
        self.range
    }

    pub fn into_parts(self) -> (EditorDocumentSnapshot, ls_types::Range) {
        (self.document, self.range)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ls_types::Position;

    use super::{DocumentRevision, EditorDocumentSnapshot, OpenDocumentSession};

    #[test]
    fn serialized_position_keeps_text_and_revision_in_one_value() {
        let input = EditorDocumentSnapshot::new(
            PathBuf::from("/workspace/src/lib.rs"),
            OpenDocumentSession::new(4),
            DocumentRevision::new(9),
            Some(3),
            "fn changed() {}".to_string(),
        )
        .with_position(Position::new(0, 3));

        let wire = serde_json::to_value(&input).expect("document input should serialize");

        assert_eq!(wire["document"]["target"]["revision"], 9);
        assert_eq!(wire["document"]["text"], "fn changed() {}");
        assert_eq!(
            wire["position"],
            serde_json::json!({ "line": 0, "character": 3 })
        );
    }
}
