//! Exact source input captured before a consumer starts rebuilding.
//!
//! Saved-project events use `CapturedSource::new`, which canonicalizes a filesystem path at the
//! capture boundary. Editor snapshots take the other route: the LSP engine first proves that a
//! document belongs to the selected generation, then reuses that generation's `SourcePath` without
//! consulting the filesystem again. Both forms keep path, text, revision, and byte length in one
//! immutable value.

use std::{path::Path, sync::Arc};

use crate::{SourceDescriptor, SourceError, SourcePath, SourceRevision};

/// One canonical Rust source value captured before project composition.
///
/// External saved-source capture canonicalizes its path. Editor capture instead reuses the
/// `SourcePath` already owned by a selected project generation, so an open buffer never has to
/// rediscover its identity from the filesystem.
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
        let text = text.into();
        let descriptor = SourceDescriptor::new(SourcePath::new(canonical_path), text.as_bytes());
        Ok(Self { descriptor, text })
    }

    pub(crate) fn from_source_path(source_path: SourcePath, text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let descriptor = SourceDescriptor::new(source_path, text.as_bytes());
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

    pub fn byte_len(&self) -> u64 {
        self.descriptor.byte_len()
    }

    pub(crate) fn descriptor(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }

    pub(crate) fn source_path(&self) -> &SourcePath {
        self.descriptor.source_path()
    }

    pub(crate) fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
}
