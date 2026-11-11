//! One generation-local saved source file and its resident or evicted text.

use std::{
    path::{Path, PathBuf},
    str,
    sync::{Arc, Mutex},
};

use rg_std::{MemoryRecorder, MemorySize};

use crate::{CapturedSource, SourceError, SourceRevision, revision::SourceDescriptor};

/// Reads one UTF-8 source file through the shared source I/O boundary.
///
/// Most analysis code should use `SourceEntry::text` instead. This function exists for callers
/// that need source I/O before a candidate captures the file, while keeping error mapping in
/// the same boundary as generation-backed reads.
pub fn read_source_text(path: &Path) -> Result<Arc<str>, SourceError> {
    SourceEntry::read_saved(path)
}

/// One exact source file used by a saved generation.
///
/// For a saved file, the descriptor never changes after capture. The resident text may disappear,
/// but a reload has to reproduce that descriptor before it can be used. For example, if an entry
/// captured `struct Before;` and disk later contains `struct After;`, `text()` returns
/// `SourceError::Stale` instead of handing the newer text to the older generation.
#[derive(Debug)]
pub struct SourceEntry {
    descriptor: SourceDescriptor,
    text: Mutex<Option<Arc<str>>>,
}

impl SourceEntry {
    pub(crate) fn saved(path: PathBuf, text: Arc<str>) -> Self {
        Self {
            descriptor: SourceDescriptor::new(path, text.as_bytes()),
            text: Mutex::new(Some(text)),
        }
    }

    pub(crate) fn saved_captured(source: &CapturedSource) -> Self {
        Self {
            descriptor: source.descriptor(),
            text: Mutex::new(Some(source.shared_text())),
        }
    }

    pub fn path(&self) -> &Path {
        self.descriptor.path()
    }

    /// Shares the canonical path with lookup indexes so they can store it without copying it.
    pub fn shared_path(&self) -> Arc<Path> {
        Arc::clone(self.descriptor.path())
    }

    pub fn revision(&self) -> SourceRevision {
        self.descriptor.revision()
    }

    pub fn byte_len(&self) -> u64 {
        self.descriptor.byte_len()
    }

    /// Returns the text that belongs to this entry's generation.
    ///
    /// Resident text can be returned immediately. An evicted file is read again, hashed, and
    /// accepted only if both its revision and byte length still match the descriptor. A mismatch
    /// is a stale-generation error, not a source update.
    pub fn text(&self) -> Result<Arc<str>, SourceError> {
        let text = self
            .text
            .lock()
            .expect("source backing lock should not be poisoned");
        if let Some(text) = &*text {
            return Ok(Arc::clone(text));
        }
        let loaded = self.read_known_saved()?;
        let actual = SourceRevision::from_bytes(loaded.as_bytes());
        if actual != self.revision()
            || u64::try_from(loaded.len()).unwrap_or(u64::MAX) != self.byte_len()
        {
            return Err(SourceError::Stale {
                path: self.path().to_path_buf(),
                expected: self.revision(),
                actual,
            });
        }
        // An accepted generation keeps the revision, while the reader owns reloaded bytes.
        Ok(loaded)
    }

    /// Drops saved text without dropping the revision needed to reload it safely.
    pub(crate) fn evict_saved_text(&self) {
        *self
            .text
            .lock()
            .expect("source backing lock should not be poisoned") = None;
    }

    /// Proves that the saved file still has the bytes captured by this entry.
    ///
    /// Candidate publication calls this even when text is resident. Resident text proves what the
    /// analysis used, but this final disk read proves the filesystem did not advance while the
    /// candidate was being built.
    pub(crate) fn validate_saved(&self) -> Result<(), SourceError> {
        let loaded = self.read_known_saved()?;
        let actual = SourceRevision::from_bytes(loaded.as_bytes());
        if actual != self.revision()
            || u64::try_from(loaded.len()).unwrap_or(u64::MAX) != self.byte_len()
        {
            return Err(SourceError::Stale {
                path: self.path().to_path_buf(),
                expected: self.revision(),
                actual,
            });
        }
        Ok(())
    }

    fn read_saved(path: &Path) -> Result<Arc<str>, SourceError> {
        let bytes = std::fs::read(path).map_err(|source| SourceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let text = str::from_utf8(&bytes).map_err(|source| SourceError::InvalidUtf8 {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Arc::from(text))
    }

    /// Read a source already named by this generation, preserving disappearance as invalidation.
    fn read_known_saved(&self) -> Result<Arc<str>, SourceError> {
        match Self::read_saved(self.path()) {
            Err(SourceError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Err(SourceError::Missing {
                    path: self.path().to_path_buf(),
                    expected: self.revision(),
                })
            }
            result => result,
        }
    }
}

impl MemorySize for SourceEntry {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.descriptor.record_memory_children(recorder);
        let text = self
            .text
            .lock()
            .expect("source backing lock should not be poisoned");
        if let Some(text) = &*text {
            text.record_memory_children(recorder);
        }
    }
}
