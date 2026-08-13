//! Immutable editor input shared by the LSP server and one analysis engine.
//!
//! The server owns live sessions and captures a complete set of applicable open documents before
//! asynchronous handling can be overtaken by another edit. The engine receives that value, never
//! a document handle it can refresh on its own. Successful results return the same lightweight
//! target and editor revision through `AnalysisInput`, which the server validates at publication.
//!
//! Two paths intentionally coexist on an editor document. `target.path` is the raw editor identity
//! used for session and publication checks. `source_path` is the identity frozen in the selected
//! project generation, so a renamed or removed open file remains analyzable without another
//! filesystem lookup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Distinguishes one open/close lifetime independently from client document versions.
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

/// Names one exact full-text value accepted during an open document session.
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

/// Names one immutable set of editor-owned open document values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EditorSnapshotRevision(u64);

impl EditorSnapshotRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Publication identity for one exact document text without carrying the text again.
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

/// Exact editor-owned value for one open Rust document.
///
/// This value deliberately carries no global editor revision. A document only becomes an analysis
/// input as part of `EditorSnapshot`, where all applicable open buffers share one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorDocumentSnapshot {
    target: TargetDocumentRevision,
    /// Frozen project-source identity selected for this open document.
    ///
    /// `target.path` remains the editor URI identity used for session and publication checks. This
    /// path is the identity used inside the selected project generation, so semantic analysis does
    /// not have to canonicalize the editor URI again after the file has been renamed or removed.
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

    /// Attach the project-source identity frozen by the document's engine route.
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
}

/// Every open Rust document applicable to one engine at a single editor revision.
///
/// Documents are sorted by path at construction so equivalent snapshots have deterministic wire
/// representation and the original editor input order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSnapshot {
    revision: EditorSnapshotRevision,
    documents: Vec<EditorDocumentSnapshot>,
}

impl EditorSnapshot {
    pub fn new(
        revision: EditorSnapshotRevision,
        mut documents: Vec<EditorDocumentSnapshot>,
    ) -> Self {
        documents.sort_by(|left, right| left.path().cmp(right.path()));
        Self {
            revision,
            documents,
        }
    }

    pub const fn revision(&self) -> EditorSnapshotRevision {
        self.revision
    }

    pub fn documents(&self) -> &[EditorDocumentSnapshot] {
        &self.documents
    }

    pub fn document(&self, target: &TargetDocumentRevision) -> Option<&EditorDocumentSnapshot> {
        self.documents
            .iter()
            .find(|document| document.target() == target)
    }
}

/// One target document selected from a complete immutable editor snapshot.
///
/// The target identity is kept separately so the cursor or range cannot be accidentally applied
/// to a sibling document. Engine code validates that the target is present before analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentAnalysisSnapshot {
    target: TargetDocumentRevision,
    editor: EditorSnapshot,
}

impl DocumentAnalysisSnapshot {
    pub fn new(target: TargetDocumentRevision, editor: EditorSnapshot) -> Self {
        Self { target, editor }
    }

    pub fn target(&self) -> &TargetDocumentRevision {
        &self.target
    }

    pub fn editor(&self) -> &EditorSnapshot {
        &self.editor
    }

    pub fn document(&self) -> Option<&EditorDocumentSnapshot> {
        self.editor.document(&self.target)
    }

    pub fn with_position(self, position: ls_types::Position) -> DocumentPositionSnapshot {
        DocumentPositionSnapshot {
            analysis: self,
            position,
        }
    }

    pub fn with_range(self, range: ls_types::Range) -> DocumentRangeSnapshot {
        DocumentRangeSnapshot {
            analysis: self,
            range,
        }
    }
}

/// Position and source selected together before asynchronous request handling can overtake edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPositionSnapshot {
    pub analysis: DocumentAnalysisSnapshot,
    pub position: ls_types::Position,
}

/// Range and source selected together before asynchronous request handling can overtake edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRangeSnapshot {
    pub analysis: DocumentAnalysisSnapshot,
    pub range: ls_types::Range,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ls_types::Position;

    use super::{
        DocumentAnalysisSnapshot, DocumentRevision, EditorDocumentSnapshot, EditorSnapshot,
        EditorSnapshotRevision, OpenDocumentSession,
    };

    #[test]
    fn serialized_position_keeps_text_and_revision_in_one_value() {
        let document = EditorDocumentSnapshot::new(
            PathBuf::from("/workspace/src/lib.rs"),
            OpenDocumentSession::new(4),
            DocumentRevision::new(9),
            Some(3),
            "fn changed() {}".to_string(),
        );
        let input = DocumentAnalysisSnapshot::new(
            document.target().clone(),
            EditorSnapshot::new(EditorSnapshotRevision::new(12), vec![document]),
        )
        .with_position(Position::new(0, 3));

        let wire = serde_json::to_value(&input).expect("document input should serialize");

        assert_eq!(wire["analysis"]["target"]["revision"], 9);
        assert_eq!(wire["analysis"]["editor"]["revision"], 12);
        assert_eq!(
            wire["analysis"]["editor"]["documents"][0]["text"],
            "fn changed() {}"
        );
        assert_eq!(
            wire["position"],
            serde_json::json!({ "line": 0, "character": 3 })
        );
    }
}
