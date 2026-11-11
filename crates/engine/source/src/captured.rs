//! Source text and path fixed together before a saved-project rebuild starts.
//!
//! `CapturedSource::new` canonicalizes the path as soon as an event supplies the text. It then
//! keeps that path, text, revision, and byte length in one value, so a later project update cannot
//! accidentally combine text from one moment with a path identity from another.

use std::{path::Path, sync::Arc};

use crate::{SourceError, SourceRevision, revision::SourceDescriptor};

/// One canonical Rust source file captured before rebuilding the saved project.
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
        let descriptor = SourceDescriptor::new(canonical_path, text.as_bytes());
        Ok(Self { descriptor, text })
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

    pub(crate) fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
}
