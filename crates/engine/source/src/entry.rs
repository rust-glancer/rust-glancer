//! One generation-local source file and its resident or evicted text.
//!
//! Saved files and dirty editor buffers share the same `SourceEntry` interface so parsing does not
//! have to decide where text comes from. Their lifetime rules are different: saved text may be
//! evicted and verified against disk later, while dirty text stays resident because the editor
//! buffer is its only authority.

use std::{
    path::Path,
    str,
    sync::{Arc, Mutex},
};

use rg_std::{MemoryRecorder, MemorySize};

use crate::{SourceDescriptor, SourceError, SourcePath, SourceRevision};

/// Reads one UTF-8 source file through the shared source I/O boundary.
///
/// Most analysis code should use `SourceEntry::text` instead. This function exists for callers
/// that need source I/O before a project inventory owns the file, while keeping error mapping in
/// the same boundary as generation-backed reads.
pub fn read_source_text(path: &Path) -> Result<Arc<str>, SourceError> {
    SourceEntry::read_saved(path)
}

/// One exact source file used by a project generation or dirty overlay.
///
/// For a saved file, the descriptor never changes after capture. The resident text may disappear,
/// but a reload has to reproduce that descriptor before it can be used. For example, if an entry
/// captured `struct Before;` and disk later contains `struct After;`, `text()` returns
/// `SourceError::Stale` instead of handing the newer text to the older generation.
#[derive(Debug)]
pub struct SourceEntry {
    descriptor: SourceDescriptor,
    backing: Mutex<SourceBacking>,
}

#[derive(Debug)]
enum SourceBacking {
    /// Saved text can be reconstructed from disk, but only at the captured revision.
    Saved { text: Option<Arc<str>> },
    /// Dirty text has no other authority, so the overlay keeps it resident.
    InMemory(Arc<str>),
}

impl SourceEntry {
    pub(crate) fn saved(path: SourcePath, text: Arc<str>) -> Self {
        Self {
            descriptor: SourceDescriptor::new(path, text.as_bytes()),
            backing: Mutex::new(SourceBacking::Saved { text: Some(text) }),
        }
    }

    pub(crate) fn in_memory(path: SourcePath, text: Arc<str>) -> Self {
        Self {
            descriptor: SourceDescriptor::new(path, text.as_bytes()),
            backing: Mutex::new(SourceBacking::InMemory(text)),
        }
    }

    pub fn path(&self) -> &Path {
        self.descriptor.path()
    }

    /// Returns the generation's shared canonical path handle for path-indexed analysis storage.
    pub fn source_path(&self) -> &SourcePath {
        self.descriptor.source_path()
    }

    pub fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }

    pub fn revision(&self) -> SourceRevision {
        self.descriptor.revision()
    }

    pub fn byte_len(&self) -> u64 {
        self.descriptor.byte_len()
    }

    pub fn is_saved(&self) -> bool {
        matches!(
            *self
                .backing
                .lock()
                .expect("source backing lock should not be poisoned"),
            SourceBacking::Saved { .. }
        )
    }

    /// Returns the text that belongs to this entry's generation.
    ///
    /// Resident saved text and dirty text can be returned immediately. An evicted saved file is
    /// different: it is read again, hashed, and accepted only if both its revision and byte length
    /// still match the descriptor. A mismatch is a stale-generation error, not a source update.
    pub fn text(&self) -> Result<Arc<str>, SourceError> {
        let mut backing = self
            .backing
            .lock()
            .expect("source backing lock should not be poisoned");
        match &mut *backing {
            SourceBacking::Saved { text: Some(text) } | SourceBacking::InMemory(text) => {
                Ok(Arc::clone(text))
            }
            SourceBacking::Saved { text } => {
                let loaded = Self::read_saved(self.path())?;
                let actual = SourceRevision::from_bytes(loaded.as_bytes());
                if actual != self.revision() {
                    return Err(SourceError::Stale {
                        path: self.path().to_path_buf(),
                        expected: self.revision(),
                        actual,
                    });
                }
                if u64::try_from(loaded.len()).unwrap_or(u64::MAX) != self.byte_len() {
                    return Err(SourceError::Stale {
                        path: self.path().to_path_buf(),
                        expected: self.revision(),
                        actual,
                    });
                }
                *text = Some(Arc::clone(&loaded));
                Ok(loaded)
            }
        }
    }

    /// Drops saved text without dropping the revision needed to reload it safely.
    pub(crate) fn evict_saved_text(&self) {
        let mut backing = self
            .backing
            .lock()
            .expect("source backing lock should not be poisoned");
        if let SourceBacking::Saved { text } = &mut *backing {
            *text = None;
        }
    }

    /// Proves that the saved file still has the bytes captured by this entry.
    ///
    /// Candidate publication calls this even when text is resident. Resident text proves what the
    /// analysis used, but this final disk read proves the filesystem did not advance while the
    /// candidate was being built.
    pub(crate) fn validate_saved(&self) -> Result<(), SourceError> {
        if !self.is_saved() {
            return Ok(());
        }
        let loaded = Self::read_saved(self.path())?;
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
}

impl MemorySize for SourceEntry {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.descriptor.record_memory_children(recorder);
        let backing = self
            .backing
            .lock()
            .expect("source backing lock should not be poisoned");

        // Saved text disappears from retained-memory reports after eviction. Dirty text remains
        // visible because dropping it would lose the only copy owned by the overlay.
        match &*backing {
            SourceBacking::Saved { text: Some(text) } | SourceBacking::InMemory(text) => {
                text.record_memory_children(recorder);
            }
            SourceBacking::Saved { text: None } => {}
        }
    }
}
