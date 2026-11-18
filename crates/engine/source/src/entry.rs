//! Exact source bytes, either reloadable from disk or retained from an editor capture.

use std::{
    path::{Path, PathBuf},
    str,
    sync::{Arc, Mutex},
};

use rg_ir_model::FileId;
use rg_std::{MemoryRecorder, MemorySize};

use crate::{CapturedSource, SourceError, SourceRevision, revision::SourceDescriptor};

/// Reads one UTF-8 source file through the shared source I/O boundary.
///
/// Most analysis code should use `SourceEntry::text` instead. This function exists for callers
/// that need source I/O before a candidate captures the file, while keeping error mapping in
/// the same boundary as reads from captured inputs.
pub fn read_source_text(path: &Path) -> Result<Arc<str>, SourceError> {
    SourceEntry::read_saved(path)
}

/// One exact source file used by an input revision.
///
/// For a saved file, the descriptor never changes after capture. The resident text may disappear,
/// but a reload has to reproduce that descriptor before it can be used. For example, if an entry
/// captured `struct Before;` and disk later contains `struct After;`, `text()` returns
/// `SourceError::Stale` instead of handing the newer text to the older input view.
/// Editor text has no disk backing and stays resident for as long as its input view is retained.
#[derive(Debug)]
pub struct SourceEntry {
    descriptor: SourceDescriptor,
    text: SourceText,
}

impl SourceEntry {
    /// Copy the small retained record out of allocations made during analysis. Saved bytes can
    /// be reloaded with revision validation; editor bytes must stay alive with this input.
    pub(crate) fn relocated(&self) -> Self {
        Self {
            descriptor: self.descriptor.relocated(),
            text: match &self.text {
                SourceText::Saved(_) => SourceText::Saved(Mutex::new(None)),
                SourceText::Editor(text) => SourceText::Editor(Arc::clone(text)),
            },
        }
    }

    pub(crate) fn init(
        file: &FileId,
        cx: &dyn ajika::InputInit,
    ) -> ajika::Result<Result<Arc<Self>, Arc<SourceError>>> {
        Self::read_capture(*file, &cx.interner())
    }

    /// Keep I/O failures in the captured value so optional includes can test readability without
    /// failing the query. Resolving the file identity can still fail the operation itself.
    pub(crate) fn read_capture(
        file: FileId,
        interner: &ajika::Interner,
    ) -> ajika::Result<Result<Arc<Self>, Arc<SourceError>>> {
        let path = file.path(interner)?;
        Ok(Self::read_saved(&path)
            .map(|text| Arc::new(Self::saved(path, text)))
            .map_err(Arc::new))
    }

    pub(crate) fn saved(path: PathBuf, text: Arc<str>) -> Self {
        Self {
            descriptor: SourceDescriptor::new(path, text.as_bytes()),
            text: SourceText::Saved(Mutex::new(Some(text))),
        }
    }

    pub(crate) fn saved_captured(source: &CapturedSource) -> Self {
        Self {
            descriptor: source.descriptor(),
            text: SourceText::Saved(Mutex::new(Some(source.shared_text()))),
        }
    }

    pub(crate) fn editor(source: &CapturedSource) -> Self {
        Self {
            descriptor: source.descriptor(),
            text: SourceText::Editor(source.shared_text()),
        }
    }

    /// Identify the captured content independently of its backing. Saving identical bytes changes
    /// their lifetime, not their stamp, so retained query results can survive the save.
    pub(crate) fn stamp(
        capture: &Result<Arc<Self>, Arc<SourceError>>,
    ) -> ajika::Result<ajika::Stamp> {
        // Evicting saved text doesn't change what a reader can observe. SourceEntry verifies the
        // revision if it has to reload those bytes from disk.
        Ok(match capture {
            Ok(entry) => entry.revision().stamp(),
            Err(_) => ajika::Stamp::of(b"unreadable Rust source"),
        })
    }

    /// Changed results depending on editor text stay out of disk storage. Read failures describe
    /// saved disk captures and therefore use the saved policy too.
    pub(crate) fn lifetime(capture: &Result<Arc<Self>, Arc<SourceError>>) -> ajika::Lifetime {
        match capture {
            Ok(entry) => match entry.text {
                SourceText::Saved(_) => ajika::Lifetime::Persistent,
                SourceText::Editor(_) => ajika::Lifetime::Session,
            },
            Err(_) => ajika::Lifetime::Persistent,
        }
    }

    /// Durability controls dependency validation, separately from storage lifetime. Once a file
    /// is dirty, later edits can skip rechecking work that depends only on unchanged saved or
    /// Cargo inputs.
    pub(crate) fn durability(capture: &Result<Arc<Self>, Arc<SourceError>>) -> ajika::Durability {
        match capture {
            Ok(entry) => match entry.text {
                SourceText::Saved(_) => ajika::Durability::Medium,
                SourceText::Editor(_) => ajika::Durability::Low,
            },
            Err(_) => ajika::Durability::Medium,
        }
    }

    pub fn path(&self) -> &Path {
        self.descriptor.path()
    }

    pub fn revision(&self) -> SourceRevision {
        self.descriptor.revision()
    }

    /// Returns the exact bytes captured for this entry.
    ///
    /// Resident text can be returned immediately. An evicted file is read again, hashed, and
    /// accepted only if both its revision and byte length still match the descriptor. A mismatch
    /// is a stale-generation error, not a source update.
    pub fn text(&self) -> Result<Arc<str>, SourceError> {
        let saved = match &self.text {
            SourceText::Editor(text) => return Ok(Arc::clone(text)),
            SourceText::Saved(text) => text,
        };
        let text = saved
            .lock()
            .expect("source backing lock should not be poisoned");
        if let Some(text) = &*text {
            return Ok(Arc::clone(text));
        }
        let loaded = self.read_known_saved()?;
        let actual = SourceRevision::from_bytes(loaded.as_bytes());
        if actual != self.revision()
            || u64::try_from(loaded.len()).unwrap_or(u64::MAX) != self.descriptor.byte_len()
        {
            return Err(SourceError::Stale {
                path: self.path().to_path_buf(),
                expected: self.revision(),
                actual,
            });
        }
        // The input keeps the revision, while the reader owns reloaded bytes.
        Ok(loaded)
    }

    /// Drops saved text without dropping the revision needed to reload it safely.
    pub(crate) fn evict_saved_text(&self) {
        if let SourceText::Saved(text) = &self.text {
            *text
                .lock()
                .expect("source backing lock should not be poisoned") = None;
        }
    }

    /// Proves that the saved file still has the bytes captured by this entry.
    ///
    /// Candidate publication calls this even when text is resident. Resident text proves what the
    /// analysis used, but this final disk read proves the filesystem did not advance while the
    /// candidate was being built. Editor captures do not describe disk and are skipped here.
    pub(crate) fn validate_saved(&self) -> Result<(), SourceError> {
        if matches!(self.text, SourceText::Editor(_)) {
            return Ok(());
        }
        let loaded = self.read_known_saved()?;
        let actual = SourceRevision::from_bytes(loaded.as_bytes());
        if actual != self.revision()
            || u64::try_from(loaded.len()).unwrap_or(u64::MAX) != self.descriptor.byte_len()
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

    /// Read a previously captured source, preserving disappearance as invalidation.
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
        let text = match &self.text {
            SourceText::Saved(text) => text.lock().expect("source backing lock").clone(),
            SourceText::Editor(text) => Some(Arc::clone(text)),
        };
        if let Some(text) = text
            && recorder.visit_shared_allocation(Arc::as_ptr(&text).cast::<()>())
        {
            text.record_memory_children(recorder);
        }
    }
}

#[derive(Debug)]
enum SourceText {
    Saved(Mutex<Option<Arc<str>>>),
    Editor(Arc<str>),
}
