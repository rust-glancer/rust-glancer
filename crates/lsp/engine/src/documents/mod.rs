//! Tracks LSP document state that sits above the saved analysis project.

mod snapshot;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) use self::snapshot::{
    DirtyDocumentSnapshot, DirtyDocumentSnapshotState, TextFingerprint,
};

/// LSP-side document freshness and live-buffer state.
///
/// The saved project remains the stable baseline, but full-sync change notifications let analysis
/// requests build a temporary dirty overlay for the queried document. Incremental-only changes
/// still mark the file dirty without exposing a text snapshot.
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentStore {
    documents: HashMap<PathBuf, DocumentState>,
}

impl DocumentStore {
    pub(crate) fn did_open(&mut self, path: PathBuf, version: Option<i32>, text: &str) {
        let fingerprint = TextFingerprint::new(text);
        self.documents.insert(
            path,
            DocumentState {
                version,
                saved: Some(fingerprint),
                live: Some(fingerprint),
                live_text: Some(Arc::from(text)),
                dirty: false,
            },
        );
    }

    /// Marks an open document dirty and returns whether this was the clean-to-dirty transition.
    pub(crate) fn did_change(
        &mut self,
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<&str>,
    ) -> DocumentChange {
        let document = self.documents.entry(path).or_default();
        let was_dirty = document.dirty;

        // `didChange` carries a monotonically increasing document version. Handlers are async, so
        // an older notification can finish after a newer one; in that case the old text must not
        // resurrect stale dirty state after save.
        if let (Some(current), Some(next)) = (document.version, version)
            && next < current
        {
            return DocumentChange::unchanged();
        }

        document.version = version;

        if let Some(full_text) = full_text {
            let live = TextFingerprint::new(full_text);
            document.live = Some(live);
            document.live_text = Some(Arc::from(full_text));
            document.dirty = document.saved != Some(live);
        } else {
            document.live = None;
            document.live_text = None;
            document.dirty = true;
        }

        DocumentChange {
            became_dirty: !was_dirty && document.dirty,
            became_clean: was_dirty && !document.dirty,
        }
    }

    pub(crate) fn did_save(&mut self, path: PathBuf, full_text: Option<&str>) {
        let document = self.documents.entry(path).or_default();
        if let Some(full_text) = full_text {
            let saved = TextFingerprint::new(full_text);
            document.saved = Some(saved);
            // `didSave` has no document version. If a newer edit has already marked the live
            // buffer dirty with different text, keep that live fingerprint instead of rolling the
            // document back to the saved snapshot.
            if !document.dirty || document.live == Some(saved) || document.live.is_none() {
                document.live = Some(saved);
                document.live_text = Some(Arc::from(full_text));
            }
        } else {
            document.saved = document.live;
        }
        document.dirty = document.live != document.saved;
    }

    pub(crate) fn mark_dirty_after_failed_save(&mut self, path: PathBuf) {
        let document = self.documents.entry(path).or_default();
        document.dirty = true;
    }

    pub(crate) fn did_close(&mut self, path: &Path) {
        self.documents.remove(path);
    }

    #[cfg(test)]
    pub(crate) fn is_dirty(&self, path: &Path) -> bool {
        self.documents
            .get(path)
            .is_some_and(|document| document.dirty)
    }

    pub(crate) fn freshness(&self, path: &Path) -> DocumentFreshness {
        self.documents
            .get(path)
            .map(DocumentFreshness::from_state)
            .unwrap_or_else(DocumentFreshness::untracked)
    }

    pub(crate) fn dirty_snapshot(&self, path: &Path) -> DirtyDocumentSnapshotState {
        let Some(document) = self.documents.get(path) else {
            return DirtyDocumentSnapshotState::Clean;
        };
        if !document.dirty {
            return DirtyDocumentSnapshotState::Clean;
        }

        let Some(fingerprint) = document.live else {
            return DirtyDocumentSnapshotState::DirtyWithoutText;
        };
        let Some(text) = &document.live_text else {
            return DirtyDocumentSnapshotState::DirtyWithoutText;
        };

        DirtyDocumentSnapshotState::Dirty(DirtyDocumentSnapshot::new(
            path.to_path_buf(),
            document.version,
            fingerprint,
            Arc::clone(text),
        ))
    }
}

#[derive(Debug, Clone, Default)]
struct DocumentState {
    version: Option<i32>,
    saved: Option<TextFingerprint>,
    live: Option<TextFingerprint>,
    live_text: Option<Arc<str>>,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentFreshness {
    tracked: bool,
    version: Option<i32>,
    dirty: bool,
    saved: Option<TextFingerprint>,
    live: Option<TextFingerprint>,
}

impl DocumentFreshness {
    fn untracked() -> Self {
        Self {
            tracked: false,
            version: None,
            dirty: false,
            saved: None,
            live: None,
        }
    }

    fn from_state(state: &DocumentState) -> Self {
        Self {
            tracked: true,
            version: state.version,
            dirty: state.dirty,
            saved: state.saved,
            live: state.live,
        }
    }

    pub(crate) fn tracked(self) -> bool {
        self.tracked
    }

    pub(crate) fn version(self) -> Option<i32> {
        self.version
    }

    pub(crate) fn dirty(self) -> bool {
        self.dirty
    }

    pub(crate) fn saved_len(self) -> Option<usize> {
        self.saved.map(TextFingerprint::len)
    }

    pub(crate) fn live_len(self) -> Option<usize> {
        self.live.map(TextFingerprint::len)
    }

    pub(crate) fn saved_hash(self) -> Option<u64> {
        self.saved.map(TextFingerprint::hash)
    }

    pub(crate) fn live_hash(self) -> Option<u64> {
        self.live.map(TextFingerprint::hash)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentChange {
    pub(crate) became_dirty: bool,
    pub(crate) became_clean: bool,
}

impl DocumentChange {
    fn unchanged() -> Self {
        Self {
            became_dirty: false,
            became_clean: false,
        }
    }
}
