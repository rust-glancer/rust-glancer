//! Immutable saved-project mutation values sent from the server to one engine.
//!
//! Existing Rust files cross this boundary as exact captured text, so queueing and rebuild time
//! cannot silently change the proposal. Filesystem paths remain only where the project domain must
//! interpret discovery, graph changes, or deletion. Editor saves additionally carry their open
//! session and document revision, keeping the proposal tied to the value accepted at ingress.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::TargetDocumentRevision;

/// Exact Rust source captured before a saved-project RPC is submitted.
///
/// The engine derives the domain `SourceRevision` when it constructs the corresponding captured
/// source. The LSP protocol deliberately carries no second fingerprint or mutable lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedSourceInput {
    path: PathBuf,
    text: String,
}

impl CapturedSourceInput {
    pub fn new(path: PathBuf, text: String) -> Self {
        Self { path, text }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_parts(self) -> (PathBuf, String) {
        (self.path, self.text)
    }
}

/// Editor save proposal captured from one exact open-session document revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveProposal {
    target: TargetDocumentRevision,
    client_version: Option<i32>,
    text: String,
}

impl SaveProposal {
    pub fn new(target: TargetDocumentRevision, client_version: Option<i32>, text: String) -> Self {
        Self {
            target,
            client_version,
            text,
        }
    }

    pub fn target(&self) -> &TargetDocumentRevision {
        &self.target
    }

    pub fn path(&self) -> &Path {
        self.target.path()
    }

    pub const fn client_version(&self) -> Option<i32> {
        self.client_version
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_parts(self) -> (TargetDocumentRevision, String) {
        (self.target, self.text)
    }
}

/// Settled watcher inputs routed to one saved-project owner.
///
/// Ordinary existing Rust files carry exact captured text in `captured_sources`. Other paths stay
/// in `fs_paths` because manifests, graph discovery, and deletion-shaped events must be resolved
/// against the filesystem by the project layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProjectChanges {
    captured_sources: Vec<CapturedSourceInput>,
    fs_paths: Vec<PathBuf>,
}

impl SavedProjectChanges {
    pub fn new(captured_sources: Vec<CapturedSourceInput>, fs_paths: Vec<PathBuf>) -> Self {
        Self {
            captured_sources,
            fs_paths,
        }
    }

    pub fn captured_sources(&self) -> &[CapturedSourceInput] {
        &self.captured_sources
    }

    pub fn fs_paths(&self) -> &[PathBuf] {
        &self.fs_paths
    }

    pub fn into_parts(self) -> (Vec<CapturedSourceInput>, Vec<PathBuf>) {
        (self.captured_sources, self.fs_paths)
    }
}
