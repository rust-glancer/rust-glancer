use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

/// LSP-side document freshness state.
///
/// The analysis engine remains save-only. This store only records whether VS Code has told us a
/// file's live buffer has diverged from the saved snapshot, so position-sensitive requests can
/// avoid returning stale answers.
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
            document.dirty = document.saved != Some(live);
        } else {
            document.live = None;
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
}

#[derive(Debug, Clone, Default)]
struct DocumentState {
    version: Option<i32>,
    saved: Option<TextFingerprint>,
    live: Option<TextFingerprint>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextFingerprint {
    len: usize,
    hash: u64,
}

impl TextFingerprint {
    fn new(text: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        Self {
            len: text.len(),
            hash: hasher.finish(),
        }
    }

    fn len(self) -> usize {
        self.len
    }

    fn hash(self) -> u64 {
        self.hash
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::DocumentStore;

    #[test]
    fn tracks_clean_to_dirty_to_clean_document_lifecycle() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut store = DocumentStore::default();

        store.did_open(path.clone(), Some(1), "fn main() {}\n");
        assert!(!store.is_dirty(&path));

        let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
        assert!(change.became_dirty);
        assert!(store.is_dirty(&path));

        let change = store.did_change(path.clone(), Some(3), Some("fn main() {\n}\n"));
        assert!(!change.became_dirty);
        assert!(store.is_dirty(&path));

        store.did_save(path.clone(), Some("fn main() {\n}\n"));
        assert!(!store.is_dirty(&path));

        store.mark_dirty_after_failed_save(path.clone());
        assert!(store.is_dirty(&path));

        store.did_close(&path);
        assert!(!store.is_dirty(&path));
    }

    #[test]
    fn ignores_delayed_change_events_that_match_saved_text() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut store = DocumentStore::default();

        store.did_open(path.clone(), Some(1), "fn main() {}\n");
        store.did_save(path.clone(), Some("fn main() {\n}\n"));
        assert!(!store.is_dirty(&path));

        let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
        assert!(!change.became_dirty);
        assert!(!store.is_dirty(&path));
    }

    #[test]
    fn ignores_out_of_order_older_change_versions() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut store = DocumentStore::default();

        store.did_open(path.clone(), Some(1), "fn main() {}\n");
        store.did_change(path.clone(), Some(3), Some("fn main() {\n    work();\n}\n"));
        store.did_save(path.clone(), Some("fn main() {\n    work();\n}\n"));
        assert!(!store.is_dirty(&path));

        let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
        assert!(!change.became_dirty);
        assert!(!change.became_clean);
        assert!(!store.is_dirty(&path));
    }

    #[test]
    fn save_keeps_document_dirty_when_a_newer_edit_already_landed() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut store = DocumentStore::default();

        store.did_open(path.clone(), Some(1), "fn main() {}\n");
        store.did_change(
            path.clone(),
            Some(3),
            Some("fn main() {\n    unsaved();\n}\n"),
        );

        store.did_save(path.clone(), Some("fn main() {\n    saved();\n}\n"));
        assert!(store.is_dirty(&path));
    }
}
