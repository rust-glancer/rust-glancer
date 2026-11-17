//! Source text and path fixed together before an editor request or saved-project rebuild.
//!
//! `CapturedSource::new` canonicalizes the path as soon as an event supplies the text. It then
//! keeps that path, text, revision, and byte length in one value, so a later project update cannot
//! accidentally combine text from one moment with a path identity from another.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{SourceError, SourceRevision, revision::SourceDescriptor};

/// One canonical Rust source file captured at the host boundary.
/// The path must already exist; virtual editor files need a separate path-identity policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSource {
    descriptor: SourceDescriptor,
    text: Arc<str>,
}

impl CapturedSource {
    pub fn new(path: impl AsRef<Path>, text: impl Into<Arc<str>>) -> Result<Self, SourceError> {
        let path = path.as_ref();
        let canonical_path = path.canonicalize().map_err(|source| SourceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self::from_source_path(canonical_path, text.into()))
    }

    /// Capture text for an identity already selected by host routing. Unlike `new`, this must not
    /// consult the filesystem again: a rename after request capture cannot change its identity.
    pub fn from_source_path(path: PathBuf, text: Arc<str>) -> Self {
        let descriptor = SourceDescriptor::new(path, text.as_bytes());
        Self { descriptor, text }
    }

    pub fn path(&self) -> &Path {
        self.descriptor.path()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn revision(&self) -> SourceRevision {
        self.descriptor.revision()
    }

    pub(crate) fn descriptor(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }

    pub(crate) fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
}
